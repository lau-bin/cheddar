use near_sdk::json_types::U128;
use near_sdk::serde::{Deserialize, Serialize};
use near_sdk::{ext_contract, AccountId};

#[ext_contract(ext_self)]
pub trait ExtSelf {
    fn return_tokens_callback(&mut self, user: AccountId, amount: U128);
    fn return_tokens_treasury_callback(&mut self, user: AccountId, amount: U128);
}

#[ext_contract(ext_ft)]
pub trait FungibleToken {
    fn ft_transfer(&mut self, receiver_id: AccountId, amount: U128, memo: Option<String>);
}

#[derive(Deserialize, Serialize)]
pub struct ContractParams {
    pub owner_id: AccountId,
    pub staked_token: AccountId,
    pub is_active: bool,
    pub closing_date: u64,
    pub total_staked: U128,
    /// Number of accounts currently registered.
    pub accounts_registered: u64,
}
