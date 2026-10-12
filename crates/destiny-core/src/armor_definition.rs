//! Raw Manifest armor metadata. No instance totals or derived armor values.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ArmorDefinition {
    /// Raw equippingBlock.uniqueLabel; empty means unconstrained, None unobserved.
    pub unique_label: Option<String>,
    /// Array position is the real socket index, including zero-type entries.
    pub socket_entries: Vec<DefinitionSocketEntry>,
    pub socket_categories: Vec<DefinitionSocketCategory>,
    pub investment_stats: Vec<InvestmentStat>,
    pub stat_group_hash: Option<u32>,
    pub plug_category_hash: Option<u32>,
    pub energy_cost: Option<PlugEnergyCost>,
    /// Raw plug energyCapacity.capacityValue, also used for aspect capacity.
    pub aspect_capacity: Option<i32>,
    pub energy_capacity_type: Option<u32>,
    pub plug_availability: Option<u32>,
    pub insertion_material_requirement_hash: Option<u32>,
    pub equipable_item_set_hash: Option<u32>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InvestmentStat {
    pub stat_type_hash: u32,
    pub value: i32,
    pub is_conditionally_active: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PlugEnergyCost {
    pub energy_cost: i32,
    pub energy_type: u32,
    pub energy_type_hash: u32,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct DefinitionSocketEntry {
    pub socket_type_hash: u32,
    pub single_initial_item_hash: u32,
    pub reusable_plug_set_hash: Option<u32>,
    pub randomized_plug_set_hash: Option<u32>,
    pub reusable_plug_items: Vec<u32>,
    pub plug_sources: Option<u32>,
    pub default_visible: Option<bool>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DefinitionSocketCategory {
    pub socket_category_hash: u32,
    pub socket_indexes: Vec<u32>,
}

impl ArmorDefinition {
    /// Bound provider-owned collections at every publication boundary.
    pub fn validate(&self) -> Result<(), crate::AppError> {
        if self
            .unique_label
            .as_ref()
            .is_some_and(|label| label.len() > 256 || label.chars().any(char::is_control))
            || self.investment_stats.len() > 64
            || self.socket_entries.len() > 32
            || self.socket_categories.len() > 32
            || self
                .socket_entries
                .iter()
                .any(|entry| entry.reusable_plug_items.len() > 1024)
            || self.socket_categories.iter().any(|category| {
                category.socket_indexes.len() > 32
                    || category
                        .socket_indexes
                        .iter()
                        .any(|index| *index as usize >= self.socket_entries.len())
            })
        {
            return Err(crate::AppError::new(
                crate::ErrorCategory::ProviderValidation,
            ));
        }
        Ok(())
    }
}
