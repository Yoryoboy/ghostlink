//! Invocation-owned build composition. Local operations do not acquire credentials.
use crate::command::{BuildApplyArgs, BuildArgs, BuildCommand, CliError};
use crate::runtime::{ProductionFoundations, ReadFoundations};
use destiny_adapters::dim_loadout::{DimBuildCodec, MAX_DIM_INPUT_BYTES};
use destiny_adapters::{BungieProfileGateway, ProfileApiKey, SystemClock};
use destiny_app::build_apply_exec::BuildGateways;
use destiny_app::builds::{BuildError, BuildReply, BuildService, LocalBuildRequest};
use destiny_app::{
    AppError, ArmorCatalogRepository, BuildCodec, BuildRepository, Clock, DestinyMembership,
    EquipGateway, EquipItemsGateway, ErrorCategory, FreshnessPolicy, InstancedPullGateway,
    InventoryRepository, ProfileGateway, ProfileSyncService, SearchTerm, StatDefinitionRepository,
    Timestamp, TransferGateway, WorldDefinitionRepository,
};

use destiny_output::builds::BuildOutput;
use std::io::Read;

fn input(value: &str) -> Result<String, CliError> {
    if value.starts_with("https://") || value.starts_with("http://") {
        return Ok(value.into());
    }
    let reader: Box<dyn Read> = if value == "-" {
        Box::new(std::io::stdin())
    } else {
        Box::new(
            std::fs::File::open(value)
                .map_err(|_| CliError::from_category(ErrorCategory::LocalStorage))?,
        )
    };
    read_input(reader)
}
pub fn read_input(reader: impl Read) -> Result<String, CliError> {
    let mut bytes = Vec::new();
    reader
        .take((MAX_DIM_INPUT_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|_| CliError::from_category(ErrorCategory::LocalStorage))?;
    if bytes.len() > MAX_DIM_INPUT_BYTES {
        return Err(CliError::invalid_request());
    }
    String::from_utf8(bytes).map_err(|_| CliError::invalid_request())
}
/// `--dry-run` plans from fresh state and never touches the gateways; otherwise the plan
/// is executed in order. Confirmation follows move/equip/apply-loadout: explicit
/// arguments are the intent, there is no prompt.
pub fn apply_build<G, I, R, C, T, E, B, P>(
    builds: &BuildService<R, C>,
    sync: &mut ProfileSyncService<G, I>,
    args: &BuildApplyArgs,
    gateways: Option<&mut BuildGateways<T, E, B, P>>,
    policy: FreshnessPolicy,
    now: Timestamp,
    pace: &mut dyn FnMut(),
) -> Result<BuildOutput, CliError>
where
    G: ProfileGateway,
    I: InventoryRepository
        + WorldDefinitionRepository
        + StatDefinitionRepository
        + ArmorCatalogRepository,
    R: BuildRepository,
    C: BuildCodec,
    T: TransferGateway,
    E: EquipGateway,
    B: EquipItemsGateway,
    P: InstancedPullGateway,
{
    let request = args.request()?;
    let capacity = destiny_adapters::equipment_capacity_policy::pinned_equipment_capacity_policy();
    Ok(BuildOutput::from_result(match (args.dry_run, gateways) {
        (false, Some(gateways)) => builds
            .apply(sync, &request, &capacity, gateways, policy, now, pace)
            .map(|applied| BuildReply::Applied(Box::new(applied))),
        (true, _) => builds
            .plan_apply(sync, &request, &capacity, policy, now)
            .map(|preview| BuildReply::ApplyPlan(Box::new(preview))),
        (false, None) => Err(BuildError::App(AppError::new(
            ErrorCategory::UnsupportedOperation,
        ))),
    }))
}
pub fn compose(arguments: &BuildArgs) -> Result<BuildOutput, CliError> {
    let local = match &arguments.command {
        BuildCommand::Import { input: value } => {
            if !std::path::Path::new(value).is_file()
                && destiny_adapters::dim_loadout::parse_dim_loadout(value)
                    == Err(destiny_app::build::DimLoadoutError::ShareLinkRequiresOnlineImport)
            {
                return Ok(BuildOutput::from_result(Err(BuildError::Codec(
                    destiny_app::build::DimLoadoutError::ShareLinkRequiresOnlineImport,
                ))));
            }
            let text = input(value)?;
            // Typed codec errors, including dim.gg, precede local setup.
            if let Err(e) = destiny_adapters::dim_loadout::parse_dim_loadout(&text) {
                return Ok(BuildOutput::from_result(Err(BuildError::Codec(e))));
            }
            Some(LocalBuildRequest::Import(text))
        }
        BuildCommand::Check(a) => {
            a.request()?;
            None
        }
        BuildCommand::Apply(a) => {
            a.request()?;
            None
        }
        BuildCommand::List => Some(LocalBuildRequest::List),
        BuildCommand::Show(s) => Some(LocalBuildRequest::Show(s.selector()?)),
        BuildCommand::Delete(s) => Some(LocalBuildRequest::Delete(s.selector()?)),
        BuildCommand::Export(s) => Some(LocalBuildRequest::Export(s.selector()?)),
        BuildCommand::FromEquipped { name, character } => {
            SearchTerm::new(name).map_err(CliError::from)?;
            SearchTerm::new(character).map_err(CliError::from)?;
            None
        }
        BuildCommand::FromLoadout(a) => {
            a.request()?;
            None
        }
    };
    let mut foundations = ProductionFoundations;
    let config = foundations.config()?;
    let repository = foundations.store(&config)?;
    let mut builds = BuildService::new(repository, DimBuildCodec);
    let now = SystemClock::new().now()?;
    if let Some(request) = local {
        let (repository, codec) = builds.into_parts();
        let metadata = repository
            .load_current()?
            .ok_or_else(|| CliError::from_category(ErrorCategory::StaleState))?
            .metadata;
        let membership = DestinyMembership::new(metadata.membership_type, metadata.membership_id)?;
        builds = BuildService::new(repository, codec);
        return Ok(BuildOutput::from_result(builds.execute_local(
            &membership,
            &request,
            now,
        )));
    }
    let inventory = foundations.store(&config)?;
    let gateway = BungieProfileGateway::new(
        foundations.transport()?,
        SystemClock::new(),
        foundations.credential_source(&config)?,
        ProfileApiKey::new(config.api_key().expose_secret())?,
        config.resolve_definition_version(&inventory)?,
    )?;
    let mut sync = ProfileSyncService::new_with_definitions(gateway, inventory);
    if let BuildCommand::Check(a) = &arguments.command {
        let request = a.request()?;
        return Ok(BuildOutput::from_result(
            builds
                .check(
                    &mut sync,
                    &request.selector,
                    request.character.as_ref(),
                    config.freshness_policy(),
                    now,
                )
                .map(|result| destiny_app::builds::BuildReply::Analysis(Box::new(result))),
        ));
    }
    if let BuildCommand::Apply(a) = &arguments.command {
        // --dry-run composes no mutation transport at all.
        let mut gateways = if a.dry_run {
            None
        } else {
            let source = foundations.credential_source(&config)?;
            let key = config.api_key().expose_secret().to_owned();
            Some(BuildGateways {
                transfer: destiny_adapters::BungieTransferGateway::new(
                    destiny_adapters::HttpTransferTransport::new()?,
                    SystemClock::new(),
                    source.clone(),
                    destiny_adapters::TransferApiKey::new(&key)?,
                )?,
                equip: destiny_adapters::BungieEquipGateway::new(
                    destiny_adapters::HttpEquipTransport::new()?,
                    SystemClock::new(),
                    source.clone(),
                    destiny_adapters::EquipApiKey::new(&key)?,
                )?,
                equip_items: destiny_adapters::BungieEquipItemsGateway::new(
                    destiny_adapters::HttpEquipItemsTransport::new()?,
                    SystemClock::new(),
                    source.clone(),
                    destiny_adapters::EquipItemsApiKey::new(&key)?,
                )?,
                pull: destiny_adapters::instanced_pull::BungieInstancedPullGateway::new(
                    destiny_adapters::instanced_pull::HttpInstancedPullTransport::new()?,
                    SystemClock::new(),
                    source,
                    destiny_adapters::instanced_pull::InstancedPullApiKey::new(&key)?,
                )?,
            })
        };
        let mut last = None;
        return apply_build(
            &builds,
            &mut sync,
            a,
            gateways.as_mut(),
            config.freshness_policy(),
            now,
            &mut || destiny_app::equip_items::pace(&mut last),
        );
    }
    let result = match &arguments.command {
        BuildCommand::FromEquipped { name, character } => builds.from_equipped(
            &mut sync,
            &destiny_app::CharacterId::new(character),
            name,
            config.freshness_policy(),
            now,
        ),
        BuildCommand::FromLoadout(a) => {
            let request = a.request()?;
            builds.from_loadout(
                &mut sync,
                &request.character,
                &request.loadout,
                config.freshness_policy(),
                now,
            )
        }
        _ => return Err(AppError::new(ErrorCategory::UnsupportedOperation).into()),
    }
    .and_then(|result| builds.reply(result));
    Ok(BuildOutput::from_result(result))
}
