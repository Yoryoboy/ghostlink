//! Raw Manifest armor metadata. No instance totals or derived armor values.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ArmorDefinition {
    pub investment_stats: Vec<InvestmentStat>,
    pub stat_group_hash: Option<u32>,
    pub plug_category_hash: Option<u32>,
    pub energy_cost: Option<PlugEnergyCost>,
    pub aspect_capacity: Option<i32>,
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

impl ArmorDefinition {
    /// Bound provider-owned collections at every publication boundary.
    pub fn validate(&self) -> Result<(), crate::AppError> {
        if self.investment_stats.len() > 64 {
            return Err(crate::AppError::new(
                crate::ErrorCategory::ProviderValidation,
            ));
        }
        Ok(())
    }
}
