//! Adds stored-build application to the existing mutation seam. The plan, executor and
//! output are the ones `ghostlink build apply` uses, so the JSON and exit class match.
use crate::schema::{
    ApplyBuildArgs, ApplyLoadoutArgs, EquipItemArgs, McpError, PullItemArgs, PullStackArgs,
    SetLockStateArgs,
};
use crate::service::{MoveItemRequest, MutationToolService};
use destiny_app::build_apply_exec::BuildGateways;
use destiny_app::builds::{BuildReply, BuildService};
use destiny_app::{
    ArmorCatalogRepository, BuildCodec, BuildRepository, Clock, EquipGateway, EquipItemsGateway,
    FreshnessPolicy, InstancedPullGateway, InventoryRepository, ProfileGateway, ProfileSyncService,
    StatDefinitionRepository, TransferGateway, WorldDefinitionRepository,
};

/// Reads and mutations are serialized by the server (the mutation lock and the shared
/// publication guard); this service only reads and writes through its own handles.
pub struct ApplyBuildMutationService<I, G, S, R, C, K, T, E, B, P> {
    pub inner: I,
    pub builds: BuildService<R, C>,
    pub sync: ProfileSyncService<G, S>,
    pub gateways: BuildGateways<T, E, B, P>,
    pub clock: K,
    pub policy: FreshnessPolicy,
    /// Monotonic, process-local pacing between provider requests.
    pub last_attempt: Option<std::time::Instant>,
}
impl<I, G, S, R, C, K, T, E, B, P> MutationToolService
    for ApplyBuildMutationService<I, G, S, R, C, K, T, E, B, P>
where
    I: MutationToolService,
    G: ProfileGateway,
    S: InventoryRepository
        + WorldDefinitionRepository
        + StatDefinitionRepository
        + ArmorCatalogRepository,
    R: BuildRepository,
    C: BuildCodec,
    K: Clock,
    T: TransferGateway,
    E: EquipGateway,
    B: EquipItemsGateway,
    P: InstancedPullGateway,
{
    fn apply_build(
        &mut self,
        args: &ApplyBuildArgs,
    ) -> Result<destiny_output::builds::BuildOutput, McpError> {
        let request = args.request()?;
        let now = self.clock.now().map_err(McpError::from)?;
        let capacity =
            destiny_adapters::equipment_capacity_policy::pinned_equipment_capacity_policy();
        let result = if args.dry_run() {
            self.builds
                .plan_apply(&mut self.sync, &request, &capacity, self.policy, now)
                .map(|preview| BuildReply::ApplyPlan(Box::new(preview)))
        } else {
            let last = &mut self.last_attempt;
            self.builds
                .apply(
                    &mut self.sync,
                    &request,
                    &capacity,
                    &mut self.gateways,
                    self.policy,
                    now,
                    &mut || destiny_app::equip_items::pace(last),
                )
                .map(|applied| BuildReply::Applied(Box::new(applied)))
        };
        Ok(destiny_output::builds::BuildOutput::from_result(result))
    }
    fn equip_items(
        &mut self,
        args: &crate::schema::EquipItemsArgs,
    ) -> Result<destiny_output::equip_items::EquipItemsOutput, McpError> {
        self.inner.equip_items(args)
    }
    fn apply_loadout(
        &mut self,
        args: &ApplyLoadoutArgs,
    ) -> Result<destiny_output::apply_loadout::ApplyLoadoutOutput, McpError> {
        self.inner.apply_loadout(args)
    }
    fn move_item(
        &mut self,
        request: &MoveItemRequest,
    ) -> Result<crate::schema::MoveItemOutput, McpError> {
        self.inner.move_item(request)
    }
    fn equip_item(
        &mut self,
        request: &EquipItemArgs,
    ) -> Result<destiny_output::equip::EquipOutput, McpError> {
        self.inner.equip_item(request)
    }
    fn set_lock_state(
        &mut self,
        request: &SetLockStateArgs,
    ) -> Result<destiny_output::lock::LockOutput, McpError> {
        self.inner.set_lock_state(request)
    }
    fn pull_item(
        &mut self,
        request: &PullItemArgs,
    ) -> Result<destiny_output::instanced_pull::InstancedPullOutput, McpError> {
        self.inner.pull_item(request)
    }
    fn pull_stack(
        &mut self,
        request: &PullStackArgs,
    ) -> Result<destiny_output::stack_pull::StackPullOutput, McpError> {
        self.inner.pull_stack(request)
    }
}
