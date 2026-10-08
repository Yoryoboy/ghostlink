//! Adds saved-loadout application to the existing mutation seam.
use crate::schema::{
    ApplyLoadoutArgs, EquipItemArgs, McpError, PullItemArgs, PullStackArgs, SetLockStateArgs,
};
use crate::service::{MoveItemRequest, MutationToolService};
use destiny_app::{
    ApplyLoadoutService, Clock, FreshnessPolicy, InventoryRepository, LoadoutActionGateway,
    LoadoutGateway, ProfileGateway, WorldDefinitionRepository,
};

pub struct ApplyLoadoutMutationService<I, G, R, A, C> {
    pub inner: I,
    pub service: ApplyLoadoutService<G, R>,
    pub action: A,
    pub clock: C,
    pub policy: FreshnessPolicy,
    /// Monotonic, process-local pacing between loadout attempts.
    pub last_attempt: Option<std::time::Instant>,
}
impl<I, G, R, A, C> MutationToolService for ApplyLoadoutMutationService<I, G, R, A, C>
where
    I: MutationToolService,
    G: ProfileGateway + LoadoutGateway,
    R: InventoryRepository + WorldDefinitionRepository,
    A: LoadoutActionGateway,
    C: Clock,
{
    fn apply_loadout(
        &mut self,
        args: &ApplyLoadoutArgs,
    ) -> Result<destiny_output::apply_loadout::ApplyLoadoutOutput, McpError> {
        let request = args.request()?;
        if let Some(last) = self.last_attempt
            && let Some(wait) = std::time::Duration::from_secs(1).checked_sub(last.elapsed())
        {
            std::thread::sleep(wait);
        }
        let now = self.clock.now().map_err(McpError::from)?;
        let result = self
            .service
            .apply(&mut self.action, &request, self.policy, now);
        self.last_attempt = Some(std::time::Instant::now());
        Ok(destiny_output::apply_loadout::ApplyLoadoutOutput::from_result(result))
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
