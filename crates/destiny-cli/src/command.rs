//! Command surface, request validation, and stable typed exits for the
//! read-only Destiny journey.
//!
//! Parsing and conversion here are pure: they never touch a provider, a
//! repository, or a database, and every request shape is decided before a
//! shared service is called. Rendering lives in [`crate::output`]; dispatch
//! through the shared application query services is deliberately not part of
//! this module.

use std::fmt;

use clap::builder::TypedValueParser;
use clap::{Args, Parser, Subcommand, ValueEnum};

use destiny_app::auth_ux::AuthorizationMode;
use destiny_app::{
    AppError, CharacterId, DefinitionSearchQuery, ErrorCategory, InspectItemRequest,
    InstanceSearchQuery, ItemHash, ItemHashFilter, ItemInspection, ItemInstanceId,
    MAX_SEARCH_LIMIT, PerkHash, Resolution, SearchInventoryRequest, SearchTerm,
};

use crate::output::OutputMode;

/// The stable code reported for a successful outcome.
pub const SUCCESS_CODE: &str = "ok";

/// The process exit status reported for a successful outcome.
pub const SUCCESS_EXIT_CODE: i32 = 0;

/// The product command surface: the shared read commands and interactive login.
///
/// The library keeps the internal `destiny_cli` name while the package and
/// parsed program are named `ghostlink`. The explicit `about` and `long_about` keep
/// this developer documentation out of the product's help output.
#[derive(Debug, Parser)]
#[command(
    override_usage = "ghostlink [OPTIONS] <COMMAND>",
    name = "ghostlink",
    version,
    about = "Destiny profile, inventory, verified item equip, and safe interactive login",
    long_about = "Read Destiny profile, inventory, and item data, equip one item with explicit character intent and verification, or authorize this machine against the provider."
)]
struct ParsedCli {
    /// Emit exactly one bounded JSON object instead of human text.
    #[arg(long, global = true)]
    pub json: bool,

    /// The read command to run, or the interactive login command.
    #[command(subcommand)]
    pub command: Option<Command>,
}

/// Public parsed command; absent subcommands are represented without breaking
/// the existing command API used by other presentation crates.
#[derive(Debug)]
pub struct Cli {
    pub json: bool,
    pub command: Command,
}
impl clap::CommandFactory for Cli {
    fn command() -> clap::Command {
        ParsedCli::command()
    }
    fn command_for_update() -> clap::Command {
        ParsedCli::command_for_update()
    }
}
impl clap::FromArgMatches for Cli {
    fn from_arg_matches(matches: &clap::ArgMatches) -> Result<Self, clap::Error> {
        let parsed = ParsedCli::from_arg_matches(matches)?;
        Ok(Self {
            json: parsed.json,
            command: parsed.command.unwrap_or(Command::Help),
        })
    }
    fn update_from_arg_matches(&mut self, matches: &clap::ArgMatches) -> Result<(), clap::Error> {
        *self = Self::from_arg_matches(matches)?;
        Ok(())
    }
}
impl Parser for Cli {}

impl Cli {
    /// The output mode selected by the global `--json` flag.
    pub fn output_mode(&self) -> OutputMode {
        if self.json {
            OutputMode::Json
        } else {
            OutputMode::Human
        }
    }

    /// Converts the parsed arguments into one validated read request.
    ///
    /// Every read validation failure is reported as [`CliError::invalid_request`],
    /// so an invalid request can never reach a shared service and can never echo
    /// the rejected value. A `login` command line carries no read at all, so this
    /// read-only conversion reports it as [`ErrorCategory::UnsupportedOperation`]
    /// instead of guessing at a read. The process boundary routes `login` to the
    /// login composition before it calls this conversion, so that outcome is only
    /// reachable from a caller that asked a read-only entry point to run login.
    pub fn into_read_request(self) -> Result<ReadRequest, CliError> {
        match self.command {
            Command::Loadouts(arguments) => {
                let character_id = SubclassArgs {
                    character: arguments.character,
                }
                .into_request()?;
                Ok(ReadRequest::Loadouts {
                    character_id,
                    detail: arguments.detail,
                })
            }
            Command::Artifact(arguments) => {
                let character_id = SubclassArgs {
                    character: arguments.character,
                }
                .into_request()?;
                Ok(ReadRequest::Artifact { character_id })
            }
            Command::ListCharacters => Ok(ReadRequest::ListCharacters),
            Command::SearchInventory(arguments) => {
                arguments.into_request().map(ReadRequest::SearchInventory)
            }
            Command::InspectItem(arguments) => {
                arguments.into_request().map(ReadRequest::InspectItem)
            }
            Command::SubclassOptions(arguments) => {
                if self.json && arguments.locked {
                    return Err(CliError::invalid_request());
                }
                let character_id = SubclassArgs {
                    character: arguments.character,
                }
                .into_request()?;
                Ok(ReadRequest::SubclassOptions {
                    character_id,
                    element: arguments.element,
                    locked: arguments.locked,
                })
            }
            Command::Subclass(arguments) => arguments.into_request().map(ReadRequest::Subclass),
            // A `login` command line carries no read, and the hidden `callback`
            // handler is not a read either, so both report the stable
            // unsupported-operation outcome instead of guessing at a read.
            Command::Equip(_)
            | Command::Sync
            | Command::SyncManifest
            | Command::WeeklyReset
            | Command::Weekly(_)
            | Command::Login(_)
            | Command::Agents(_)
            | Command::Setup
            | Command::Help
            | Command::Handler(_)
            | Command::Callback(_) => {
                Err(CliError::from_category(ErrorCategory::UnsupportedOperation))
            }
        }
    }
}

#[derive(Debug, Args)]
pub struct AgentsArgs {
    #[command(subcommand)]
    pub command: AgentsCommand,
}

#[derive(Debug, Subcommand)]
pub enum AgentsCommand {
    Status,
    Install(AgentSelection),
    Uninstall(AgentSelection),
}

#[derive(Clone, Copy, Debug, ValueEnum)]
pub enum AgentName {
    Claude,
    Codex,
    Opencode,
    Pi,
}

#[derive(Debug, Args)]
pub struct AgentSelection {
    #[arg(long, required = true, num_args = 1..)]
    pub agent: Vec<AgentName>,
    #[arg(long)]
    pub skill: bool,
    #[arg(long)]
    pub mcp: bool,
}

/// The read commands and the interactive login command this journey exposes.
#[derive(Debug, Subcommand)]
pub enum Command {
    #[command(skip)]
    Help,
    /// Interactively connect agents and optionally sign in to Bungie.
    Setup,
    /// Inspect or explicitly install integrations for supported AI agents.
    Agents(AgentsArgs),
    /// Equip one item, transferring first when needed, then refresh and verify.
    #[command(name = "equip")]
    Equip(EquipArgs),

    /// Force profile acquisition and publish the local inventory projection.
    #[command(name = "sync")]
    Sync,

    /// Download and publish the public English Manifest item definitions.
    #[command(name = "sync-manifest")]
    SyncManifest,

    /// Show the public weekly reset using stored Manifest definitions.
    #[command(name = "weekly-reset")]
    WeeklyReset,

    /// Show reliable weekly content and progress for one character.
    #[command(name = "weekly")]
    Weekly(WeeklyArgs),

    /// List the characters in the current profile projection.
    #[command(name = "list-characters")]
    ListCharacters,

    /// Search definitions and, optionally, concrete inventory instances.
    #[command(name = "search-inventory")]
    SearchInventory(Box<SearchInventoryArgs>),

    /// Inspect exactly one concrete item instance.
    #[command(name = "inspect-item")]
    InspectItem(InspectItemArgs),

    /// Show equipped subclass configurations for one or all characters.
    #[command(name = "subclass")]
    Subclass(SubclassArgs),

    /// Show available subclass options and their unlock status.
    #[command(name = "subclass-options")]
    SubclassOptions(SubclassOptionsArgs),

    /// Show seasonal artifact points, power, and active perks.
    #[command(name = "artifact")]
    Artifact(ArtifactArgs),

    /// Show saved in-game loadouts for one or all characters.
    #[command(name = "loadouts")]
    Loadouts(LoadoutsArgs),

    /// Authorize this machine against the provider.
    ///
    /// The human owns the Bungie application registration and the browser
    /// authorization. This command prints the authorization URL, opens the
    /// browser when the mode allows it, and then reads the custom-scheme callback
    /// URI through the desktop handler or terminal paste. Auto offers handler
    /// installation only with terminal consent. It never asks for, reads, or
    /// prints a password, a client secret, or a token value.
    #[command(name = "login")]
    Login(LoginArgs),

    /// Inspect, install, or remove the ghostlink:// desktop handler.
    Handler(HandlerArgs),

    /// Internal custom-scheme callback handler.
    ///
    /// The installed desktop entry invokes `ghostlink callback <uri>`, which
    /// forwards one operating-system callback to a waiting `ghostlink login`
    /// process over the per-user rendezvous. It is hidden because the operating
    /// system, not a person, runs it. The URI is never rendered, logged, or
    /// persisted. A successful run means only that the URI was delivered to a
    /// waiting login; the login, not this handler, owns and reports the
    /// authorization outcome, so exit success here is delivery, not authorization.
    #[command(name = "callback", hide = true)]
    Callback(CallbackArgs),
}

/// How the authorization page is presented to the human.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, ValueEnum)]
pub enum LoginMode {
    /// Detect the available surface and choose `browser` or `headless`.
    #[default]
    Auto,
    /// Print the authorization URL and open it in the local system browser.
    Browser,
    /// Print the authorization URL without opening a browser.
    Headless,
}

impl LoginMode {
    /// The shared authorization mode this flag names.
    pub const fn authorization_mode(self) -> AuthorizationMode {
        match self {
            Self::Auto => AuthorizationMode::Auto,
            Self::Browser => AuthorizationMode::Browser,
            Self::Headless => AuthorizationMode::Headless,
        }
    }
}

/// Arguments for `login`.
///
/// The mode controls whether the authorization URL opens in a local browser.
/// Auto callback delivery detects the desktop handler and offers installation
/// with terminal consent; explicit paste and handler bypass that policy.
#[derive(Debug, Args)]
pub struct LoginArgs {
    /// Whether to open the authorization URL in the local system browser.
    #[arg(long, value_enum, default_value = "auto", value_name = "MODE")]
    pub mode: LoginMode,

    /// How the custom-scheme callback reaches this process.
    #[arg(long, value_enum, default_value = "auto", value_name = "CALLBACK")]
    pub callback: CallbackMode,
}

/// Arguments for desktop handler management.
#[derive(Debug, Args)]
pub struct HandlerArgs {
    #[command(subcommand)]
    pub command: HandlerCommand,
}

/// Explicit desktop handler operations; install itself is consent.
#[derive(Clone, Copy, Debug, Subcommand)]
pub enum HandlerCommand {
    /// Inspect registration and known desktop pitfalls without changing it.
    Status,
    /// Install this executable as the ghostlink:// handler.
    Install,
    /// Remove this handler and restore the previous association.
    Uninstall,
}

/// Arguments for the internal `callback` handler.
///
/// The value is the single URI the operating system handed to this process. On
/// the desktop that argument is a `%u` expansion, so the one-use authorization
/// code is transiently present in this process's argv; that exposure is inherent
/// to desktop custom-scheme handling and is minimized here by validating,
/// forwarding once, and never rendering or storing the value.
///
/// The handler's exit status reports delivery only. The waiting login validates
/// the callback state and exchanges the code, so authorization success and
/// failure are never observed or reported here.
#[derive(Debug, Args)]
pub struct CallbackArgs {
    /// The exact callback URI the operating system handed to this process.
    #[arg(value_name = "CALLBACK_URI")]
    pub callback_uri: String,
}

/// How the custom-scheme callback reaches the waiting login process.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, ValueEnum)]
pub enum CallbackMode {
    /// Detect the handler and offer installation when interactive (default).
    #[default]
    Auto,
    /// Read the callback from the terminal (available everywhere).
    Paste,
    /// Receive the callback from the installed desktop handler over the
    /// per-user Unix-socket rendezvous (Linux only).
    Handler,
}

impl CallbackMode {
    /// The stable machine label for this callback strategy.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Paste => "paste",
            Self::Handler => "handler",
        }
    }
}

/// Arguments for `search-inventory`.
///
/// Definition filters select Manifest candidates and constrain owned hits.
/// An unfiltered search returns candidates only; any filter also searches
/// concrete instances without silently choosing one.
#[derive(Debug, Args)]
pub struct SearchInventoryArgs {
    /// Match socketed or selectable perks and mods (default: socketed only).
    #[arg(long)]
    pub include_available: bool,
    /// Case-insensitive substring of the definition name.
    #[arg(long, value_name = "TEXT")]
    pub text: Option<String>,

    /// Exact definition item type, matched case-insensitively.
    #[arg(long = "type", value_name = "TYPE")]
    pub item_type: Option<String>,

    /// Exact stored tier, matched case-insensitively; unknown names match nothing.
    #[arg(long, value_name = "NAME")]
    pub tier: Option<String>,

    /// Instance location: vault or character (combined with --character using AND).
    #[arg(long, value_name = "LOCATION")]
    pub location: Option<String>,

    /// Instance equipped state: true or false.
    #[arg(long, value_name = "BOOL")]
    pub equipped: Option<String>,

    /// Definition tag that must be present; repeat for several tags.
    #[arg(long = "tag", value_name = "TAG")]
    pub tag: Vec<String>,

    /// Damage type: kinetic, arc, solar, void, stasis, strand (case-insensitive).
    #[arg(long, value_name = "NAME", value_parser = parse_element)]
    pub element: Option<String>,

    /// Slot: kinetic, energy, power, helmet, gauntlets, chest, legs, classitem, other, unknown.
    #[arg(long, value_name = "NAME", value_parser = parse_slot)]
    pub slot: Option<String>,

    /// Class: titan, hunter, warlock, any, unknown (case-insensitive).
    #[arg(long = "class", value_name = "NAME", value_parser = parse_class)]
    pub class_type: Option<String>,

    /// Stat comparison (Name>=N, Name<=N, Name>N, Name<N, Name=N); repeat to require all.
    #[arg(long = "stat", value_name = "COND")]
    pub stat: Vec<String>,

    /// Sort owned hits by stat, descending by default; append :asc for ascending.
    #[arg(long, value_name = "STAT[:asc|desc]")]
    pub sort: Option<String>,

    /// Minimum instance power; items without power do not match.
    #[arg(long, value_name = "N")]
    pub min_power: Option<u16>,

    /// Restrict instance matches to one definition hash; repeat for several.
    #[arg(long = "item-hash", value_name = "HASH")]
    pub item_hash: Vec<i64>,

    /// Weapon-perk name: exact first, then substring; repeat to require all (max 8).
    #[arg(long = "perk", value_name = "NAME")]
    pub perk: Vec<String>,

    /// Armor-mod name: exact first, then substring; repeat to require all (max 8).
    #[arg(long = "mod", value_name = "NAME")]
    pub mod_name: Vec<String>,

    /// Socketed armor-archetype name: exact first, then substring; typos get suggestions.
    #[arg(long, value_name = "NAME")]
    pub archetype: Option<String>,

    /// Restrict instance matches to one exact perk hash.
    #[arg(long = "perk-hash", value_name = "HASH")]
    pub perk_hash: Option<u32>,

    /// Restrict instance matches to the character with this exact identifier.
    #[arg(long = "character", value_name = "CHARACTER_ID")]
    pub character: Option<String>,

    /// Maximum number of definition candidates and instance hits.
    #[arg(long, value_name = "N", default_value_t = 100)]
    pub limit: usize,
}

impl SearchInventoryArgs {
    /// Builds the two shared query shapes this command maps onto.
    fn into_request(self) -> Result<SearchInventoryRequest, CliError> {
        let definitions = DefinitionSearchQuery {
            text: search_term(self.text.as_deref(), "--text")?,
            item_type: search_term(self.item_type.as_deref(), "--type")?,
            tier: search_term(self.tier.as_deref(), "--tier")?,
            tags: self
                .tag
                .iter()
                .map(|value| SearchTerm::new(value).map_err(|_| CliError::invalid_argument("--tag", "must be non-empty, at most 256 characters, and contain no prohibited control characters")))
                .collect::<Result<Vec<_>, _>>()?,
            element: self
                .element
                .as_deref()
                .map(str::parse)
                .transpose()
                .map_err(|_| CliError::invalid_argument("--element", "expected one of kinetic, arc, solar, void, stasis, strand"))?,
            slot: self
                .slot
                .as_deref()
                .map(str::parse)
                .transpose()
                .map_err(|_| CliError::invalid_argument("--slot", "expected one of kinetic, energy, power, helmet, gauntlets, chest, legs, classitem, other, unknown"))?,
            class_type: self
                .class_type
                .as_deref()
                .map(str::parse)
                .transpose()
                .map_err(|_| CliError::invalid_argument("--class", "expected one of titan, hunter, warlock, any, unknown"))?,
            limit: self.limit,
        };
        definitions.validate().map_err(CliError::query_validation)?;

        let instances = self.instance_query(&definitions)?;
        Ok(SearchInventoryRequest {
            definitions,
            instances,
        })
    }

    /// Builds an owned search when any definition or instance filter is supplied.
    fn instance_query(
        &self,
        definitions: &DefinitionSearchQuery,
    ) -> Result<Option<InstanceSearchQuery>, CliError> {
        let no_instance_filter = !self.include_available
            && self.stat.is_empty()
            && self.sort.is_none()
            && self.item_hash.is_empty()
            && self.perk_hash.is_none()
            && self.perk.is_empty()
            && self.mod_name.is_empty()
            && self.archetype.is_none()
            && self.character.is_none()
            && self.location.is_none()
            && self.equipped.is_none();
        if no_instance_filter && !definitions.has_filters() && self.min_power.is_none() {
            return Ok(None);
        }

        let character_id = match &self.character {
            Some(value) => {
                validate_identity(value, "--character")?;
                Some(CharacterId::new(value.as_str()))
            }
            None => None,
        };
        let item_hashes = if self.item_hash.is_empty() {
            ItemHashFilter::Any
        } else {
            ItemHashFilter::AnyOf(self.item_hash.iter().copied().map(ItemHash::new).collect())
        };
        let query = InstanceSearchQuery {
            sort: self
                .sort
                .as_deref()
                .map(str::parse)
                .transpose()
                .map_err(CliError::query_validation)?,
            include_available: self.include_available,
            item_hashes,
            perk_hash: self.perk_hash.map(PerkHash::new),
            perk_name: search_terms(&self.perk, "--perk")?,
            mod_name: search_terms(&self.mod_name, "--mod")?,
            archetype: search_term(self.archetype.as_deref(), "--archetype")?,
            stats: self
                .stat
                .iter()
                .map(|value| value.parse())
                .collect::<Result<Vec<_>, _>>()
                .map_err(CliError::query_validation)?,
            character_id,
            location: self
                .location
                .as_deref()
                .map(str::parse)
                .transpose()
                .map_err(CliError::query_validation)?,
            equipped: self
                .equipped
                .as_deref()
                .map(|value| {
                    value.parse::<bool>().map_err(|_| {
                        CliError::invalid_argument("--equipped", "expected true or false")
                    })
                })
                .transpose()?,
            definitions: definitions
                .has_filters()
                .then(|| Box::new(definitions.clone())),
            min_power: self.min_power,
            limit: self.limit,
        };
        query.validate().map_err(CliError::query_validation)?;
        Ok(Some(query))
    }
}

fn search_terms(
    values: &[String],
    argument: &'static str,
) -> Result<Option<Vec<SearchTerm>>, CliError> {
    if values.is_empty() {
        return Ok(None);
    }
    values
        .iter()
        .map(|value| search_term(Some(value), argument).map(|term| term.expect("provided term")))
        .collect::<Result<Vec<_>, _>>()
        .map(Some)
}

// Infer the core enum types from the shared query rather than duplicating
// their parsing rules in the command adapter.
fn validate_filter(value: &str, filter: &str) -> Result<String, String> {
    let mut query = DefinitionSearchQuery {
        text: None,
        item_type: None,
        tier: None,
        tags: Vec::new(),
        element: None,
        slot: None,
        class_type: None,
        limit: MAX_SEARCH_LIMIT,
    };
    match filter {
        "element" => query.element = Some(value.parse().map_err(|_| "accepted values: kinetic, arc, solar, void, stasis, strand")?),
        "slot" => query.slot = Some(value.parse().map_err(|_| "accepted values: kinetic, energy, power, helmet, gauntlets, chest, legs, classitem, other, unknown")?),
        _ => query.class_type = Some(value.parse().map_err(|_| "accepted values: titan, hunter, warlock, any, unknown")?),
    }
    Ok(value.to_owned())
}

fn parse_element(value: &str) -> Result<String, String> {
    validate_filter(value, "element")
}
fn parse_slot(value: &str) -> Result<String, String> {
    validate_filter(value, "slot")
}
fn parse_class(value: &str) -> Result<String, String> {
    validate_filter(value, "class")
}

/// Weekly accepts opaque IDs and uniquely matched character classes.
#[derive(Debug, Args)]
pub struct WeeklyArgs {
    /// Show modifiers grouped by derived names.
    #[arg(long, conflicts_with = "debug")]
    pub details: bool,

    /// Show raw entries, hashes, and Manifest diagnostics.
    #[arg(long, conflicts_with = "details")]
    pub debug: bool,

    /// Exact ID or hunter|titan|warlock (case-insensitive, unique class).
    /// Defaults to strictly highest light; ties or missing light use deterministic first.
    #[arg(long, value_name = "ID_OR_CLASS")]
    pub character: Option<String>,
}

/// Human weekly verbosity; JSON retains its independent complete contract.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum WeeklyLevel {
    #[default]
    Default,
    Details,
    Debug,
}

impl WeeklyArgs {
    pub fn output_level(&self, mode: OutputMode) -> Result<WeeklyLevel, CliError> {
        if (self.details || self.debug) && mode == OutputMode::Json {
            return Err(CliError::invalid_argument(
                if self.details { "--details" } else { "--debug" },
                "cannot be used with --json",
            ));
        }
        Ok(if self.debug {
            WeeklyLevel::Debug
        } else if self.details {
            WeeklyLevel::Details
        } else {
            WeeklyLevel::Default
        })
    }

    pub fn character_id(&self) -> Result<Option<CharacterId>, CliError> {
        self.character
            .as_deref()
            .map(|value| {
                validate_identity(value, "--character")?;
                Ok(CharacterId::new(value))
            })
            .transpose()
    }
}

/// One invocation explicitly authorizes equipping exactly one item.
#[derive(Debug, Args)]
pub struct EquipArgs {
    /// Concrete instance id or item name; ambiguous names require an instance id.
    #[arg(value_name = "ITEM")]
    pub item: String,
    /// Required explicit target: exact id or hunter|titan|warlock (case-insensitive).
    #[arg(long, value_name = "CHARACTER")]
    pub character: String,
}
impl EquipArgs {
    /// Pure validation runs before configuration, credentials, or network access.
    pub fn validate(&self) -> Result<(), CliError> {
        validate_identity(&self.item, "ITEM")?;
        destiny_app::ItemSelector::semantic(&self.item).map_err(CliError::from)?;
        validate_identity(&self.character, "--character")?;
        Ok(())
    }
}

/// Arguments for `loadouts`; JSON always includes the full result.
#[derive(Debug, Args)]
pub struct LoadoutsArgs {
    /// Exact ID or hunter|titan|warlock (case-insensitive, unique class).
    #[arg(long, value_name = "ID_OR_CLASS")]
    pub character: Option<String>,
    /// Additionally list each item and its configured plugs (ignored for JSON).
    #[arg(long)]
    pub detail: bool,
}

/// Arguments for `artifact`; omitted selection includes every character.
#[derive(Debug, Args)]
pub struct ArtifactArgs {
    /// Exact ID or hunter|titan|warlock (case-insensitive, unique class).
    #[arg(long, value_name = "ID_OR_CLASS")]
    pub character: Option<String>,
}

/// Arguments for `subclass`; omitted selection includes every character.
#[derive(Debug, Args)]
pub struct SubclassArgs {
    /// Exact ID or hunter|titan|warlock (case-insensitive, unique class).
    #[arg(long, value_name = "ID_OR_CLASS")]
    pub character: Option<String>,
}

/// Arguments for `subclass-options`; unlock status may be unknown for other classes.
#[derive(Debug, Args)]
pub struct SubclassOptionsArgs {
    /// Exact ID or hunter|titan|warlock (case-insensitive, unique class).
    #[arg(long, value_name = "ID_OR_CLASS")]
    pub character: Option<String>,

    /// Filter by subclass element.
    #[arg(long, value_parser = clap::builder::PossibleValuesParser::new(["arc", "solar", "void", "stasis", "strand", "prismatic"])
        .map(|value| value.parse::<destiny_app::SubclassElement>().expect("validated subclass element")))]
    pub element: Option<destiny_app::SubclassElement>,

    /// Show only locked options for characters with reliable status (text only).
    #[arg(long, conflicts_with = "json")]
    pub locked: bool,
}

impl SubclassArgs {
    fn into_request(self) -> Result<Option<CharacterId>, CliError> {
        self.character
            .map(|value| {
                validate_identity(&value, "--character")?;
                Ok(CharacterId::new(value))
            })
            .transpose()
    }
}

#[cfg(test)]
mod subclass_tests {
    use super::*;

    #[test]
    fn loadouts_parses_selection_detail_and_json() {
        for (args, selector, detail) in [
            (vec!["ghostlink", "loadouts"], None, false),
            (
                vec!["ghostlink", "loadouts", "--character", "Hunter", "--detail"],
                Some("Hunter"),
                true,
            ),
            (
                vec![
                    "ghostlink",
                    "--json",
                    "loadouts",
                    "--character",
                    "123",
                    "--detail",
                ],
                Some("123"),
                true,
            ),
        ] {
            assert_eq!(
                Cli::try_parse_from(args)
                    .unwrap()
                    .into_read_request()
                    .unwrap(),
                ReadRequest::Loadouts {
                    character_id: selector.map(CharacterId::new),
                    detail
                }
            );
        }
        for value in [" ", "id\nvalue"] {
            assert_eq!(
                Cli::try_parse_from(["ghostlink", "loadouts", "--character", value])
                    .unwrap()
                    .into_read_request()
                    .unwrap_err()
                    .code(),
                "invalid_request"
            );
        }
        assert!(Cli::try_parse_from(["ghostlink", "loadouts", "--tiers"]).is_err());
    }

    #[test]
    fn artifact_parses_optional_character_and_rejects_tiers() {
        for (args, expected) in [
            (vec!["ghostlink", "artifact"], None),
            (
                vec!["ghostlink", "artifact", "--character", "Hunter"],
                Some("Hunter"),
            ),
            (
                vec!["ghostlink", "--json", "artifact", "--character", "123"],
                Some("123"),
            ),
        ] {
            assert_eq!(
                Cli::try_parse_from(args)
                    .unwrap()
                    .into_read_request()
                    .unwrap(),
                ReadRequest::Artifact {
                    character_id: expected.map(CharacterId::new)
                }
            );
        }
        for value in [" ", "id\nvalue"] {
            assert_eq!(
                Cli::try_parse_from(["ghostlink", "artifact", "--character", value])
                    .unwrap()
                    .into_read_request()
                    .unwrap_err()
                    .code(),
                "invalid_request"
            );
        }
        for args in [
            vec!["ghostlink", "artifact", "--tiers"],
            vec!["ghostlink", "--json", "artifact", "--tiers"],
            vec!["ghostlink", "artifact", "--tiers", "--json"],
        ] {
            assert!(Cli::try_parse_from(args).is_err());
        }
    }

    #[test]
    fn subclass_options_parses_filters_and_rejects_invalid_combinations() {
        for args in [
            vec!["ghostlink", "subclass-options"],
            vec![
                "ghostlink",
                "subclass-options",
                "--character",
                "Hunter",
                "--element",
                "prismatic",
                "--locked",
            ],
            vec![
                "ghostlink",
                "--json",
                "subclass-options",
                "--character",
                "123",
            ],
        ] {
            assert!(
                Cli::try_parse_from(args)
                    .unwrap()
                    .into_read_request()
                    .is_ok()
            );
        }
        for element in ["arc", "solar", "void", "stasis", "strand", "prismatic"] {
            assert!(
                Cli::try_parse_from(["ghostlink", "subclass-options", "--element", element])
                    .is_ok()
            );
        }
        for args in [
            vec!["ghostlink", "subclass-options", "--element", "invalid"],
            vec!["ghostlink", "--json", "subclass-options", "--locked"],
            vec!["ghostlink", "subclass-options", "--locked", "--json"],
        ] {
            assert!(match Cli::try_parse_from(args) {
                Err(_) => true,
                Ok(cli) => cli.into_read_request().is_err(),
            });
        }
    }

    #[test]
    fn subclass_parses_optional_character() {
        for (args, expected) in [
            (vec!["ghostlink", "subclass"], None),
            (
                vec!["ghostlink", "--json", "subclass", "--character", "Hunter"],
                Some("Hunter"),
            ),
            (
                vec!["ghostlink", "subclass", "--character", "123"],
                Some("123"),
            ),
        ] {
            let request = Cli::try_parse_from(args)
                .unwrap()
                .into_read_request()
                .unwrap();
            assert_eq!(
                request,
                ReadRequest::Subclass(expected.map(CharacterId::new))
            );
        }
        assert!(
            Cli::try_parse_from(["ghostlink", "subclass", "--character", " "])
                .unwrap()
                .into_read_request()
                .is_err()
        );
    }
}

/// Arguments for `inspect-item`.
#[derive(Debug, Args)]
pub struct InspectItemArgs {
    /// The concrete item instance identifier to inspect.
    ///
    /// A definition hash is never accepted here: a name or hash query resolves
    /// through `search-inventory` first, and only a concrete instance identifier
    /// reaches exact inspection.
    #[arg(value_name = "INSTANCE_ID")]
    pub instance_id: String,
}

impl InspectItemArgs {
    fn into_request(self) -> Result<InspectItemRequest, CliError> {
        validate_identity(&self.instance_id, "INSTANCE_ID")?;
        Ok(InspectItemRequest {
            instance_id: ItemInstanceId::new(self.instance_id),
        })
    }
}

/// One validated read request derived from the parsed command line.
///
/// Each variant carries the exact shared request type, so the CLI adds no query
/// concept of its own and a malformed value can never reach a service.
// A single bounded command request; keep the shared request directly accessible.
#[allow(clippy::large_enum_variant)]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ReadRequest {
    ListCharacters,
    SearchInventory(SearchInventoryRequest),
    InspectItem(InspectItemRequest),
    Subclass(Option<CharacterId>),
    SubclassOptions {
        character_id: Option<CharacterId>,
        element: Option<destiny_app::SubclassElement>,
        locked: bool,
    },
    Artifact {
        character_id: Option<CharacterId>,
    },
    Loadouts {
        character_id: Option<CharacterId>,
        detail: bool,
    },
}

/// A stable machine code and process exit status for one failure outcome.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ErrorOutcome {
    /// The stable machine-readable code.
    pub code: &'static str,
    /// The process exit status.
    pub exit_code: i32,
}

/// Maps one application error category to its stable CLI outcome.
///
/// The match is exhaustive over [`ErrorCategory`] by construction: adding a new
/// category without extending this table is a compile error, so no category can
/// silently fall through to a generic or successful outcome.
pub fn category_outcome(category: ErrorCategory) -> ErrorOutcome {
    let (code, exit_code) = match category {
        ErrorCategory::AuthorizationRequired => ("authorization_required", 10),
        ErrorCategory::AuthorizationDenied => ("authorization_denied", 10),
        ErrorCategory::AlreadyAuthorizing => ("authorization_state", 10),
        ErrorCategory::AlreadyAuthorized => ("authorization_state", 10),
        ErrorCategory::CsrfStateMismatch => ("authorization_state", 10),
        ErrorCategory::ReauthorizationRequired => ("reauthorization_required", 11),
        ErrorCategory::ProviderAuthentication => ("reauthorization_required", 11),
        ErrorCategory::ProviderRateLimit => ("provider_rate_limit", 20),
        ErrorCategory::ProviderUnavailable => ("provider_unavailable", 21),
        ErrorCategory::CallbackTimeout => ("provider_unavailable", 21),
        ErrorCategory::ProviderValidation => ("invalid_request", 2),
        ErrorCategory::DefinitionsUnavailable => ("definitions_unavailable", 33),
        ErrorCategory::UnknownStatName => ("unknown_stat_name", 2),
        ErrorCategory::UnknownSearchName => ("unknown_search_name", 2),
        ErrorCategory::AmbiguousSearchName => ("ambiguous_search_name", 2),
        ErrorCategory::StaleState => ("stale_state", 30),
        ErrorCategory::AmbiguousMatch => ("ambiguous_match", 31),
        ErrorCategory::UnsupportedOperation => ("unsupported_operation", 40),
        ErrorCategory::PartialFailure => ("partial_failure", 41),
        ErrorCategory::VerificationFailed => ("verification_failed", 42),
        ErrorCategory::CredentialStoreUnavailable => ("credential_store_unavailable", 50),
        ErrorCategory::LocalStorage => ("local_storage", 51),
        ErrorCategory::ConfigurationError => ("configuration_error", 52),
        ErrorCategory::ClockOverflow => ("internal_error", 70),
        ErrorCategory::PlanIdExhausted => ("internal_error", 70),
    };
    ErrorOutcome { code, exit_code }
}

/// A stable, sanitized CLI failure.
///
/// [`CliError::code`] and [`CliError::exit_code`] are the machine contract, and
/// the [`fmt::Display`] message is the single sanitized line written for a
/// human. Neither ever echoes a rejected value, a provider payload, a
/// credential, or a header.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CliError {
    kind: FailureKind,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum FailureKind {
    /// CLI or shared request validation rejected the arguments.
    InvalidRequest,
    /// An exact inspection found no instance.
    ItemNotFound,
    /// A shared application service reported a typed error.
    Application(AppError),
}

impl CliError {
    /// The CLI/shared request validation failure.
    pub const fn invalid_request() -> Self {
        Self {
            kind: FailureKind::InvalidRequest,
        }
    }

    pub fn invalid_argument(argument: &'static str, reason: impl AsRef<str>) -> Self {
        Self::from_app(AppError::invalid_argument(argument, reason))
    }

    fn query_validation(error: AppError) -> Self {
        let message = error.to_string();
        for (name, flag) in [
            ("location", "--location"),
            ("limit", "--limit"),
            ("perk", "--perk"),
            ("mod", "--mod"),
            ("tag", "--tag"),
            ("stat", "--stat"),
            ("sort", "--sort"),
            ("item-hash", "--item-hash"),
        ] {
            if let Some(reason) = message.strip_prefix(&format!("invalid {name}: ")) {
                return Self::invalid_argument(flag, reason);
            }
        }
        Self::from_app(error)
    }

    /// The explicit not-found outcome of an exact inspection.
    pub const fn item_not_found() -> Self {
        Self {
            kind: FailureKind::ItemNotFound,
        }
    }

    /// Wraps one shared application error.
    pub const fn from_app(error: AppError) -> Self {
        Self {
            kind: FailureKind::Application(error),
        }
    }

    /// Wraps one application error category as a fresh, value-free error.
    pub const fn from_category(category: ErrorCategory) -> Self {
        Self::from_app(AppError::new(category))
    }

    /// The stable machine-readable code.
    pub fn code(&self) -> &'static str {
        match self.kind {
            FailureKind::InvalidRequest => "invalid_request",
            FailureKind::ItemNotFound => "item_not_found",
            FailureKind::Application(ref error) => category_outcome(error.category()).code,
        }
    }

    /// The stable process exit status.
    pub fn exit_code(&self) -> i32 {
        match self.kind {
            FailureKind::InvalidRequest => 2,
            FailureKind::ItemNotFound => 32,
            FailureKind::Application(ref error) => category_outcome(error.category()).exit_code,
        }
    }
}

impl fmt::Display for CliError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.kind {
            FailureKind::InvalidRequest => formatter.write_str("the request arguments are invalid"),
            FailureKind::ItemNotFound => {
                formatter.write_str("the requested item instance was not found")
            }
            FailureKind::Application(ref error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for CliError {}

impl From<AppError> for CliError {
    fn from(error: AppError) -> Self {
        Self::from_app(error)
    }
}

/// Classifies one exact inspection resolution into a renderable inspection or a
/// stable typed failure.
///
/// A missing instance is the explicit `item_not_found` outcome rather than an
/// empty success. The shared service fails closed on an ambiguous instance
/// identifier, but the CLI still refuses to guess if one ever reaches here.
pub fn inspection_outcome(
    resolution: Resolution<ItemInspection>,
) -> Result<ItemInspection, CliError> {
    match resolution {
        Resolution::Unique(inspection) => Ok(inspection),
        Resolution::Missing => Err(CliError::item_not_found()),
        Resolution::Ambiguous(_) => Err(CliError::from_category(ErrorCategory::AmbiguousMatch)),
    }
}

/// Validates and normalizes one optional free-text search term.
fn search_term(
    value: Option<&str>,
    argument: &'static str,
) -> Result<Option<SearchTerm>, CliError> {
    value
        .map(SearchTerm::new)
        .transpose()
        .map_err(|_| CliError::invalid_argument(argument, "must be non-empty, at most 256 characters, and contain no prohibited control characters"))
}

/// Rejects an empty, whitespace-only, or control-bearing identifier value.
///
/// Identity values are opaque and are used verbatim, so this check never trims
/// or rewrites one. It only refuses a value that could not name anything and a
/// value that could break the line-oriented human output.
fn validate_identity(value: &str, argument: &'static str) -> Result<(), CliError> {
    if value.trim().is_empty() || value.chars().any(char::is_control) {
        return Err(CliError::invalid_argument(
            argument,
            "must be non-empty and contain no control characters",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn include_available_flag_is_accepted() {
        use clap::Parser;
        assert!(
            super::Cli::try_parse_from([
                "ghostlink",
                "search-inventory",
                "--perk",
                "Kill Clip",
                "--include-available"
            ])
            .is_ok()
        );
    }
    use super::{
        CallbackMode, Cli, CliError, Command, ErrorOutcome, ReadRequest, SUCCESS_CODE,
        SUCCESS_EXIT_CODE, category_outcome, inspection_outcome,
    };
    use crate::output::OutputMode;
    use clap::Parser;
    use destiny_app::{
        ErrorCategory, ItemHashFilter, ItemInspection, ItemInstanceId, ItemLocation,
        MAX_HASH_FILTERS, MAX_SEARCH_LIMIT, MAX_SEARCH_TEXT_LENGTH, MAX_TAG_FILTERS, Resolution,
        SearchInventoryRequest, SyncMetadata, Timestamp,
    };

    const ALL_CATEGORIES: [ErrorCategory; 25] = [
        ErrorCategory::AuthorizationRequired,
        ErrorCategory::ReauthorizationRequired,
        ErrorCategory::AlreadyAuthorizing,
        ErrorCategory::AlreadyAuthorized,
        ErrorCategory::AuthorizationDenied,
        ErrorCategory::CsrfStateMismatch,
        ErrorCategory::CredentialStoreUnavailable,
        ErrorCategory::ProviderAuthentication,
        ErrorCategory::ProviderRateLimit,
        ErrorCategory::ProviderUnavailable,
        ErrorCategory::ProviderValidation,
        ErrorCategory::DefinitionsUnavailable,
        ErrorCategory::UnknownStatName,
        ErrorCategory::UnknownSearchName,
        ErrorCategory::AmbiguousSearchName,
        ErrorCategory::StaleState,
        ErrorCategory::AmbiguousMatch,
        ErrorCategory::UnsupportedOperation,
        ErrorCategory::PartialFailure,
        ErrorCategory::VerificationFailed,
        ErrorCategory::LocalStorage,
        ErrorCategory::ConfigurationError,
        ErrorCategory::CallbackTimeout,
        ErrorCategory::ClockOverflow,
        ErrorCategory::PlanIdExhausted,
    ];

    fn expectation(category: ErrorCategory) -> ErrorOutcome {
        match category {
            ErrorCategory::AuthorizationRequired => ErrorOutcome {
                code: "authorization_required",
                exit_code: 10,
            },
            ErrorCategory::AuthorizationDenied => ErrorOutcome {
                code: "authorization_denied",
                exit_code: 10,
            },
            ErrorCategory::AlreadyAuthorizing | ErrorCategory::AlreadyAuthorized => ErrorOutcome {
                code: "authorization_state",
                exit_code: 10,
            },
            ErrorCategory::CsrfStateMismatch => ErrorOutcome {
                code: "authorization_state",
                exit_code: 10,
            },
            ErrorCategory::ReauthorizationRequired | ErrorCategory::ProviderAuthentication => {
                ErrorOutcome {
                    code: "reauthorization_required",
                    exit_code: 11,
                }
            }
            ErrorCategory::ProviderRateLimit => ErrorOutcome {
                code: "provider_rate_limit",
                exit_code: 20,
            },
            ErrorCategory::ProviderUnavailable | ErrorCategory::CallbackTimeout => ErrorOutcome {
                code: "provider_unavailable",
                exit_code: 21,
            },
            ErrorCategory::ProviderValidation => ErrorOutcome {
                code: "invalid_request",
                exit_code: 2,
            },
            ErrorCategory::UnknownSearchName => ErrorOutcome {
                code: "unknown_search_name",
                exit_code: 2,
            },
            ErrorCategory::AmbiguousSearchName => ErrorOutcome {
                code: "ambiguous_search_name",
                exit_code: 2,
            },
            ErrorCategory::UnknownStatName => ErrorOutcome {
                code: "unknown_stat_name",
                exit_code: 2,
            },
            ErrorCategory::DefinitionsUnavailable => ErrorOutcome {
                code: "definitions_unavailable",
                exit_code: 33,
            },
            ErrorCategory::StaleState => ErrorOutcome {
                code: "stale_state",
                exit_code: 30,
            },
            ErrorCategory::AmbiguousMatch => ErrorOutcome {
                code: "ambiguous_match",
                exit_code: 31,
            },
            ErrorCategory::UnsupportedOperation => ErrorOutcome {
                code: "unsupported_operation",
                exit_code: 40,
            },
            ErrorCategory::PartialFailure => ErrorOutcome {
                code: "partial_failure",
                exit_code: 41,
            },
            ErrorCategory::VerificationFailed => ErrorOutcome {
                code: "verification_failed",
                exit_code: 42,
            },
            ErrorCategory::CredentialStoreUnavailable => ErrorOutcome {
                code: "credential_store_unavailable",
                exit_code: 50,
            },
            ErrorCategory::LocalStorage => ErrorOutcome {
                code: "local_storage",
                exit_code: 51,
            },
            ErrorCategory::ConfigurationError => ErrorOutcome {
                code: "configuration_error",
                exit_code: 52,
            },
            ErrorCategory::ClockOverflow | ErrorCategory::PlanIdExhausted => ErrorOutcome {
                code: "internal_error",
                exit_code: 70,
            },
        }
    }

    fn parse<I>(arguments: I) -> Cli
    where
        I: IntoIterator,
        I::Item: AsRef<str>,
    {
        let mut argv = vec!["destiny-cli".to_string()];
        argv.extend(
            arguments
                .into_iter()
                .map(|argument| argument.as_ref().to_string()),
        );
        Cli::try_parse_from(argv).expect("arguments should parse")
    }

    fn rejected<I>(arguments: I) -> CliError
    where
        I: IntoIterator,
        I::Item: AsRef<str>,
    {
        parse(arguments)
            .into_read_request()
            .expect_err("arguments should be rejected")
    }

    fn search_request(request: ReadRequest) -> SearchInventoryRequest {
        match request {
            ReadRequest::SearchInventory(request) => request,
            other => panic!("expected a search request, found {other:?}"),
        }
    }

    #[test]
    fn weekly_output_levels_are_local_and_validated() {
        for flag in ["--details", "--debug"] {
            let cli = parse(["weekly", flag]);
            match cli.command {
                Command::Weekly(args) => {
                    assert_eq!(
                        args.output_level(OutputMode::Human).unwrap(),
                        if flag == "--details" {
                            super::WeeklyLevel::Details
                        } else {
                            super::WeeklyLevel::Debug
                        }
                    );
                    assert_eq!(
                        args.output_level(OutputMode::Json).unwrap_err().code(),
                        "invalid_request"
                    );
                }
                _ => panic!("expected weekly"),
            }
            for argv in [
                vec!["ghostlink", "--json", "weekly", flag],
                vec!["ghostlink", "weekly", flag, "--json"],
            ] {
                let cli = Cli::try_parse_from(argv).unwrap();
                let mode = cli.output_mode();
                if let Command::Weekly(args) = cli.command {
                    assert_eq!(args.output_level(mode).unwrap_err().exit_code(), 2);
                }
            }
            assert!(Cli::try_parse_from(["ghostlink", "weekly-reset", flag]).is_err());
            assert!(Cli::try_parse_from(["ghostlink", "list-characters", flag]).is_err());
        }
        assert!(Cli::try_parse_from(["ghostlink", "weekly", "--details", "--debug"]).is_err());
    }

    #[test]
    fn weekly_parses_exact_character_and_default() {
        match parse(["weekly"]).command {
            Command::Weekly(args) => assert_eq!(args.character_id().unwrap(), None),
            _ => panic!("expected weekly"),
        }
        for selector in ["", " ", "id\nvalue"] {
            match parse(["weekly", "--character", selector]).command {
                Command::Weekly(args) => {
                    assert_eq!(args.character_id().unwrap_err().code(), "invalid_request");
                }
                _ => panic!("expected weekly"),
            }
        }
        let cli = parse(["weekly", "--character", "warlock", "--json"]);
        assert_eq!(cli.output_mode(), OutputMode::Json);
        match cli.command {
            Command::Weekly(args) => assert_eq!(
                args.character_id().unwrap(),
                Some(destiny_app::CharacterId::new("warlock"))
            ),
            _ => panic!("expected weekly"),
        }
        let help = Cli::try_parse_from(["ghostlink", "--help"])
            .unwrap_err()
            .to_string();
        assert!(
            help.lines()
                .any(|line| line.trim_start().starts_with("weekly "))
        );
        let help = Cli::try_parse_from(["ghostlink", "weekly", "--help"])
            .unwrap_err()
            .to_string();
        assert!(help.contains("ID_OR_CLASS"));
        assert!(help.contains("case-insensitive"));
        assert!(help.contains("strictly highest light"));
        assert!(help.contains("deterministic first"));
    }

    #[test]
    fn weekly_reset_parses_with_global_json_and_public_help() {
        for arguments in [
            vec!["weekly-reset"],
            vec!["--json", "weekly-reset"],
            vec!["weekly-reset", "--json"],
        ] {
            let cli = parse(arguments.clone());
            assert_eq!(
                cli.output_mode() == OutputMode::Json,
                arguments.contains(&"--json")
            );
            assert_eq!(
                cli.into_read_request().unwrap_err().code(),
                "unsupported_operation"
            );
        }
        let help = Cli::try_parse_from(["ghostlink", "--help"])
            .unwrap_err()
            .to_string();
        assert!(help.contains("weekly-reset"));
        assert_eq!(
            Cli::try_parse_from(["ghostlink", "weekly-reset", "--help"])
                .unwrap_err()
                .kind(),
            clap::error::ErrorKind::DisplayHelp
        );
    }

    #[test]
    fn sync_parses_global_json_and_rejects_extra_arguments() {
        for args in [vec!["sync"], vec!["--json", "sync"], vec!["sync", "--json"]] {
            let cli = parse(args.clone());
            assert!(matches!(cli.command, Command::Sync));
            assert_eq!(
                cli.output_mode() == OutputMode::Json,
                args.contains(&"--json")
            );
            assert_eq!(
                cli.into_read_request().unwrap_err().code(),
                "unsupported_operation"
            );
        }
        for args in [
            vec!["ghostlink", "sync", "extra"],
            vec!["ghostlink", "sync", "--force"],
            vec!["ghostlink", "sync", "--character", "123"],
        ] {
            assert!(Cli::try_parse_from(args).is_err());
        }
    }

    #[test]
    fn sync_manifest_parses_and_has_public_help() {
        let cli = parse(["sync-manifest"]);
        assert_eq!(
            cli.into_read_request().unwrap_err().code(),
            "unsupported_operation"
        );
        let help = Cli::try_parse_from(["ghostlink", "--help"])
            .unwrap_err()
            .to_string();
        assert!(help.contains("sync-manifest"));
        let help = Cli::try_parse_from(["ghostlink", "sync-manifest", "--help"]).unwrap_err();
        assert_eq!(help.kind(), clap::error::ErrorKind::DisplayHelp);
    }

    #[test]
    fn subcommands_parse_into_their_arguments() {
        assert!(matches!(
            parse(["list-characters"]).command,
            Command::ListCharacters
        ));

        let search = parse(["search-inventory", "--text", "gjallarhorn", "--limit", "5"]);
        match search.command {
            Command::SearchInventory(arguments) => {
                assert_eq!(arguments.text.as_deref(), Some("gjallarhorn"));
                assert_eq!(arguments.limit, 5);
                assert!(arguments.tag.is_empty());
            }
            other => panic!("unexpected command: {other:?}"),
        }

        let inspect = parse(["inspect-item", "inst-1"]);
        match inspect.command {
            Command::InspectItem(arguments) => assert_eq!(arguments.instance_id, "inst-1"),
            other => panic!("unexpected command: {other:?}"),
        }
    }

    #[test]
    fn the_hidden_callback_handler_parses_and_is_never_a_read() {
        let parsed = parse(["callback", "ghostlink://oauth/callback?state=s&code=c"]);
        match parsed.command {
            Command::Callback(arguments) => {
                assert_eq!(
                    arguments.callback_uri,
                    "ghostlink://oauth/callback?state=s&code=c"
                );
            }
            other => panic!("unexpected command: {other:?}"),
        }
        // A read entry point refuses the handler rather than inventing a read.
        let error = rejected(["callback", "ghostlink://oauth/callback?state=s&code=c"]);
        assert_eq!(error.code(), "unsupported_operation");
        assert_eq!(error.exit_code(), 40);
    }

    #[test]
    fn the_login_callback_strategy_parses_and_defaults_to_auto() {
        match parse(["login"]).command {
            Command::Login(arguments) => assert_eq!(arguments.callback, CallbackMode::Auto),
            other => panic!("unexpected command: {other:?}"),
        }
        match parse(["login", "--callback", "handler"]).command {
            Command::Login(arguments) => assert_eq!(arguments.callback, CallbackMode::Handler),
            other => panic!("unexpected command: {other:?}"),
        }
        assert_eq!(CallbackMode::Paste.as_str(), "paste");
        assert_eq!(CallbackMode::Handler.as_str(), "handler");
    }

    #[test]
    fn json_selection_is_global_and_defaults_to_human() {
        assert_eq!(parse(["list-characters"]).output_mode(), OutputMode::Human);
        assert_eq!(
            parse(["--json", "list-characters"]).output_mode(),
            OutputMode::Json
        );
        assert_eq!(
            parse(["search-inventory", "--json"]).output_mode(),
            OutputMode::Json
        );
    }

    #[test]
    fn search_defaults_to_100_with_a_higher_query_bound() {
        let request = search_request(
            parse(["search-inventory"])
                .into_read_request()
                .expect("an unfiltered search is valid"),
        );
        assert_eq!(request.definitions.limit, 100);
        assert_eq!(request.instances, None);
    }

    #[test]
    fn search_attribute_filters_build_owned_queries() {
        let request = search_request(
            parse([
                "search-inventory",
                "--element",
                "SoLaR",
                "--slot",
                "Energy",
                "--class",
                "Hunter",
                "--min-power",
                "2000",
            ])
            .into_read_request()
            .expect("valid filters"),
        );
        assert_eq!(
            request.definitions.element.map(|v| v.as_str()),
            Some("solar")
        );
        assert_eq!(request.definitions.slot.map(|v| v.as_str()), Some("energy"));
        assert_eq!(
            request.definitions.class_type.map(|v| v.as_str()),
            Some("hunter")
        );
        let instances = request.instances.expect("owned query");
        assert_eq!(instances.min_power, Some(2000));
        assert_eq!(instances.definitions.as_deref(), Some(&request.definitions));
        for args in [
            vec!["--type", "Hand Cannon"],
            vec!["--text", "ace"],
            vec!["--tag", "exotic"],
            vec!["--min-power", "2000"],
        ] {
            let mut argv = vec!["search-inventory"];
            argv.extend(args);
            assert!(
                search_request(parse(argv).into_read_request().unwrap())
                    .instances
                    .is_some()
            );
        }
        for (flag, accepted) in [
            ("--element", "strand"),
            ("--slot", "classitem"),
            ("--class", "warlock"),
        ] {
            let error = Cli::try_parse_from(["ghostlink", "search-inventory", flag, "invalid"])
                .unwrap_err();
            assert!(error.to_string().contains(accepted), "{error}");
        }
        assert!(
            Cli::try_parse_from(["ghostlink", "search-inventory", "--min-power", "65536"]).is_err()
        );
    }

    #[test]
    fn definition_filters_are_normalized_into_the_shared_queries() {
        let request = search_request(
            parse([
                "search-inventory",
                "--text",
                "  Gjallar   Horn ",
                "--type",
                "Rocket Launcher",
                "--tag",
                "Exotic",
                "--tag",
                "  Solar ",
            ])
            .into_read_request()
            .expect("valid definition filters"),
        );

        assert_eq!(
            request.definitions.text.as_ref().map(|term| term.as_str()),
            Some("gjallar horn")
        );
        assert_eq!(
            request
                .definitions
                .item_type
                .as_ref()
                .map(|term| term.as_str()),
            Some("rocket launcher")
        );
        assert_eq!(
            request
                .definitions
                .tags
                .iter()
                .map(|term| term.as_str())
                .collect::<Vec<_>>(),
            ["exotic", "solar"]
        );
        assert_eq!(
            request.instances.unwrap().definitions.as_deref(),
            Some(&request.definitions)
        );
    }

    #[test]
    fn instance_filters_build_one_exact_instance_query() {
        let request = search_request(
            parse([
                "search-inventory",
                "--item-hash",
                "1",
                "--item-hash",
                "2",
                "--perk-hash",
                "5001",
                "--character",
                "warlock",
                "--limit",
                "7",
            ])
            .into_read_request()
            .expect("valid instance filters"),
        );

        let instances = request.instances.expect("instance filters were supplied");
        assert_eq!(instances.limit, 7);
        assert_eq!(instances.perk_hash.map(|hash| hash.value()), Some(5001));
        assert_eq!(
            instances
                .character_id
                .as_ref()
                .map(|character| character.as_str()),
            Some("warlock")
        );
        match instances.item_hashes {
            ItemHashFilter::AnyOf(hashes) => {
                assert_eq!(
                    hashes.iter().map(|hash| hash.value()).collect::<Vec<_>>(),
                    [1, 2]
                );
            }
            ItemHashFilter::Any => panic!("an explicit hash filter was supplied"),
        }
    }

    #[test]
    fn perk_only_search_matches_every_definition_hash() {
        let request = search_request(
            parse(["search-inventory", "--perk-hash", "42"])
                .into_read_request()
                .expect("valid perk filter"),
        );

        let instances = request
            .instances
            .expect("a perk filter is an instance filter");
        assert_eq!(instances.item_hashes, ItemHashFilter::Any);
    }

    #[test]
    fn invalid_arguments_are_rejected_as_invalid_requests() {
        let oversized_text = "a".repeat(MAX_SEARCH_TEXT_LENGTH + 1);
        let mut too_many_tags: Vec<String> = vec!["search-inventory".to_string()];
        let mut too_many_hashes: Vec<String> = vec!["search-inventory".to_string()];
        for _ in 0..=MAX_TAG_FILTERS {
            too_many_tags.push("--tag".to_string());
            too_many_tags.push("exotic".to_string());
        }
        for _ in 0..=MAX_HASH_FILTERS {
            too_many_hashes.push("--item-hash".to_string());
            too_many_hashes.push("1".to_string());
        }

        let mut cases: Vec<Vec<&str>> = vec![
            vec!["search-inventory", "--limit", "0"],
            vec!["search-inventory", "--limit", "1001"],
            vec!["search-inventory", "--text", "   "],
            vec!["search-inventory", "--type", ""],
            vec!["search-inventory", "--tag", "  "],
            vec!["search-inventory", "--character", ""],
            vec!["search-inventory", "--character", "war\nlock"],
            vec!["inspect-item", ""],
            vec!["inspect-item", "  "],
            vec!["inspect-item", "inst\n1"],
        ];
        cases.push({
            let mut case = vec!["search-inventory", "--text"];
            case.push(oversized_text.as_str());
            case
        });
        cases.push(too_many_tags.iter().map(String::as_str).collect());
        cases.push(too_many_hashes.iter().map(String::as_str).collect());

        for case in cases {
            let error = rejected(case.iter().copied());
            assert_eq!(error.code(), "invalid_request", "case: {case:?}");
            assert_eq!(error.exit_code(), 2, "case: {case:?}");
            let argument = case
                .get(1)
                .filter(|arg| arg.starts_with("--"))
                .copied()
                .unwrap_or("INSTANCE_ID");
            assert!(
                error
                    .to_string()
                    .starts_with(&format!("invalid {argument}: ")),
                "case: {case:?}: {error}"
            );
        }
    }

    #[test]
    fn inputs_at_the_shared_bounds_are_accepted() {
        let mut at_limit: Vec<String> = vec![
            "search-inventory".to_string(),
            "--limit".to_string(),
            MAX_SEARCH_LIMIT.to_string(),
        ];
        for _ in 0..MAX_TAG_FILTERS {
            at_limit.push("--tag".to_string());
            at_limit.push("exotic".to_string());
        }
        for _ in 0..MAX_HASH_FILTERS {
            at_limit.push("--item-hash".to_string());
            at_limit.push("1".to_string());
        }
        let at_limit: Vec<&str> = at_limit.iter().map(String::as_str).collect();

        let request = search_request(
            parse(at_limit)
                .into_read_request()
                .expect("the shared bounds themselves are accepted"),
        );
        assert_eq!(request.definitions.limit, MAX_SEARCH_LIMIT);
        assert_eq!(request.definitions.tags.len(), MAX_TAG_FILTERS);
        let instances = request
            .instances
            .expect("hash filters are instance filters");
        match instances.item_hashes {
            ItemHashFilter::AnyOf(hashes) => assert_eq!(hashes.len(), MAX_HASH_FILTERS),
            ItemHashFilter::Any => panic!("hash filters were supplied"),
        }
    }

    #[test]
    fn rejected_values_never_appear_in_the_failure_message() {
        let error = rejected(["search-inventory", "--text", "secret-sentinel-\u{7}-value"]);
        let diagnostic = error.to_string();
        assert!(!diagnostic.contains("secret-sentinel"));
        assert_eq!(error.code(), "invalid_request");
    }

    #[test]
    fn the_error_matrix_is_stable_over_every_current_category() {
        assert_eq!(
            ALL_CATEGORIES.len(),
            25,
            "a new ErrorCategory needs a matrix entry and a test expectation"
        );
        for category in ALL_CATEGORIES {
            let outcome = category_outcome(category);
            let expected = expectation(category);
            assert_eq!(outcome, expected, "category: {category:?}");
        }

        let mapped = rejected(["inspect-item", ""]);
        assert_eq!(mapped.code(), "invalid_request");
        assert_eq!(
            CliError::from_category(ErrorCategory::ProviderValidation).code(),
            "invalid_request"
        );
        assert_eq!(SUCCESS_CODE, "ok");
        assert_eq!(SUCCESS_EXIT_CODE, 0);
    }

    #[test]
    fn missing_inspection_is_a_typed_not_found_outcome() {
        let missing = inspection_outcome(Resolution::<ItemInspection>::Missing)
            .expect_err("a missing instance is not a success");
        assert_eq!(missing.code(), "item_not_found");
        assert_eq!(missing.exit_code(), 32);
        assert_eq!(
            missing.to_string(),
            "the requested item instance was not found"
        );

        let ambiguous = inspection_outcome(Resolution::Ambiguous(Vec::new()))
            .expect_err("an ambiguous instance is not a success");
        assert_eq!(ambiguous.code(), "ambiguous_match");
        assert_eq!(ambiguous.exit_code(), 31);
    }

    #[test]
    fn unique_inspection_is_returned_unchanged() {
        let inspection = ItemInspection {
            perk_names: Vec::new(),
            mod_names: Vec::new(),
            archetype: None,
            available_perks: Vec::new(),
            available_mods: Vec::new(),
            stats: Vec::new(),
            definition: destiny_app::ItemDefinition {
                hash: destiny_app::ItemHash::new(1),
                bucket_hash: None,
                name: "Gjallarhorn".to_string(),
                item_type: None,
                tier: None,
                element: None,
                slot: None,
                class_type: None,
                plug_category: None,
                tags: Vec::new(),
                version: destiny_app::DefinitionVersion::new("manifest-v1"),
            },
            instance: destiny_app::ItemInstance {
                available_plugs: Vec::new(),
                instance_id: ItemInstanceId::new("inst-1"),
                item_hash: destiny_app::ItemHash::new(1),
                location: ItemLocation::Vault,
                quantity: 1,
                equipped: false,
                locked: false,
                power: None,
                stats: Vec::new(),
                plugs: Vec::new(),
                perks: Vec::new(),
            },
            metadata: destiny_app::QueryMetadata {
                sync: SyncMetadata {
                    dataset: destiny_app::Dataset::ProfileInventory,
                    checked_at: Timestamp::new(100),
                    source: destiny_app::SourceMarker::Profile,
                    revision: destiny_app::SyncRevision::new(1),
                    definition_version: destiny_app::DefinitionVersion::new("manifest-v1"),
                },
                freshness: destiny_app::FreshnessStatus::Fresh,
            },
        };

        let returned = inspection_outcome(Resolution::Unique(inspection.clone()))
            .expect("a unique instance is a success");
        assert_eq!(returned, inspection);
    }
}
