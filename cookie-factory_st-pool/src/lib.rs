use near_sdk::borsh::{self, BorshDeserialize, BorshSerialize};
use near_sdk::collections::UnorderedMap;
use near_sdk::json_types::{ValidAccountId, U128};
use near_sdk::{
    assert_one_yocto, env, log, near_bindgen, AccountId, PanicOnDefault, Promise, PromiseResult,
};

pub mod constants;
pub mod errors;
pub mod interfaces;
pub mod vault;

use crate::interfaces::*;
use crate::{constants::*, errors::*, vault::*};

near_sdk::setup_alloc!();

/// Staking pool for cookie-factory NFT game
#[near_bindgen]
#[derive(BorshDeserialize, BorshSerialize, PanicOnDefault)]
pub struct Contract {
    pub owner_id: AccountId,
    /// NEP-141 token for staking
    pub staking_token: AccountId,
    /// if farming is opened
    pub is_active: bool,
    /// date after when no more deposit operations are allowed
    pub closing_date: u64,
    /// user vaults
    pub vaults: UnorderedMap<AccountId, Vault>,
    /// total amount of tokens deposited
    total: u128,
    /// total number of accounts currently registered.
    pub accounts_registered: u64,
    /// Treasury address - a destination for the collected stNear.
    pub treasury: AccountId,
    /// if this stacked tokens will be returned
    pub returnable: bool,
}

#[near_bindgen]
impl Contract {
    /// Initializes the contract with the account where the NEP-141 token contract resides
    #[init]
    pub fn new(
        owner_id: ValidAccountId,
        staked_token: ValidAccountId,
        treasury: ValidAccountId,
        returnable: bool,
        closing_date: u64
    ) -> Self {
        Self {
            owner_id: owner_id.into(),
            staking_token: staked_token.into(),
            is_active: true,
            vaults: UnorderedMap::new(b"v".to_vec()),
            total: 0,
            accounts_registered: 0,
            treasury: treasury.into(),
            returnable,
            closing_date
        }
    }

    // ************ //
    // view methods //

    pub fn get_contract_params(&self) -> ContractParams {
        ContractParams {
            owner_id: self.owner_id.clone(),
            staked_token: self.staking_token.clone(),
            is_active: self.is_active,
            closing_date: self.closing_date,
            total_staked: self.total.into(),
            accounts_registered: self.accounts_registered,
        }
    }

    /// Returns amount of staked tokens
    pub fn status(&self, account_id: AccountId) -> U128 {
        return match self.vaults.get(&account_id) {
            Some(v) => v.staked.into(),
            None => U128::from(0),
        };
    }

    // ******************* //
    // transaction methods //

    /// Unstakes given amount of tokens and transfers it back to the user.
    /// If amount equals to the amount staked then we close the account.
    /// NOTE: account once closed must re-register to stake again.
    /// Returns amount of staked tokens left (still staked) after the call.
    /// Panics if the caller doesn't stake anything or if he doesn't have enough staked tokens.
    /// Requires 1 yNEAR payment for wallet 2FA.
    #[payable]
    pub fn unstake(&mut self, amount: U128) -> U128 {
        self.assert_is_active();
        if self.returnable == false {
            self.assert_not_closed();
        }
        assert_one_yocto();
        let amount_u = amount.0;
        let a = env::predecessor_account_id();
        let mut v = self.get_vault(&a);
        assert!(amount_u <= v.staked, "{}", ERR30_NOT_ENOUGH_STAKE);
        if amount_u == v.staked {
            //unstake all => close -- simplify UI
            self.close();
            return v.staked.into();
        }
        v.staked -= amount_u;
        self.total -= amount_u;

        self.vaults.insert(&a, &v);
        self.return_tokens(a, amount);
        return v.staked.into();
    }

    /// Unstakes everything and close the account.
    /// Requires 1 yNEAR payment for wallet validation.
    #[payable]
    pub fn close(&mut self) {
        self.assert_is_active();
        if self.returnable == false {
            self.assert_not_closed();
        }
        assert_one_yocto();
        let a = env::predecessor_account_id();
        let v = self.get_vault(&a);
        log!("Closing {} account", &a);
        // if user doesn't stake anything then we can make a shortcut,
        // remove the account and return storage deposit.
        if v.staked == 0 {
            self.vaults.remove(&a);
            Promise::new(a.clone()).transfer(NEAR_BALANCE);
            return;
        }

        self.total -= v.staked;

        // We remove the vault but we will try to recover in a callback if the transfer fail
        self.vaults.remove(&a);
        self.accounts_registered -= 1;

        self.return_tokens(a.clone(), v.staked.clone().into());
    }

    pub fn get_registered_accounts(&self, from_index: u64, limit: u64) -> Vec<String>{
        let mut tmp = vec![];
        let keys = self.vaults.keys_as_vector();
        let end = std::cmp::min(from_index + limit, keys.len());
        for i in from_index..end {
            tmp.push(keys.get(i).unwrap());
        }
        tmp
    }

    // ******************* //
    // management          //

    /// Transfers all tokens to treasury
    pub fn withdraw_tokens(&self) {
        assert!(!self.returnable, "this tokens are returnable");
        assert!(self.is_contract_closed(), "contract is not closed");
        self.assert_owner();

        ext_ft::ft_transfer(
            self.treasury.clone(),
            self.total.into(),
            Some("withdrawing all to treasury".to_string()),
            &self.staking_token,
            1,
            GAS_FOR_FT_TRANSFER,
        )
        .then(ext_self::return_tokens_treasury_callback(
            self.treasury.clone(),
            self.total.into(),
            &env::current_account_id(),
            0,
            GAS_FOR_MINT_CALLBACK,
        ));
    }

    /// Opens or closes smart contract operations. When the contract is not active, it will
    /// reject some functions
    pub fn set_active(&mut self, is_open: bool) {
        self.assert_owner();
        self.is_active = is_open;
    }

    /// set the date after when deposit operations are not allowed 
    pub fn set_closing_date(&mut self, date: u64) {
        self.assert_owner();
        self.closing_date = date;
    }

    /*****************
     * internal methods */

    fn create_account(&mut self, user: &AccountId, staked: u128) {
        self.vaults.insert(&user, &Vault { staked });
        self.accounts_registered += 1;
    }

    fn assert_is_active(&self) {
        assert!(self.is_active, "contract is not active");
    }

    /// transfers staked tokens back to the user
    #[inline]
    fn return_tokens(&mut self, user: AccountId, amount: U128) -> Promise {
        return ext_ft::ft_transfer(
            user.clone(),
            amount.0.into(),
            Some("unstaking".to_string()),
            &self.staking_token,
            1,
            GAS_FOR_FT_TRANSFER,
        )
        .then(ext_self::return_tokens_callback(
            user,
            amount,
            &env::current_account_id(),
            0,
            GAS_FOR_MINT_CALLBACK,
        ));
    }

    #[private]
    pub fn return_tokens_callback(&mut self, user: AccountId, amount: U128) {
        match env::promise_result(0) {
            PromiseResult::NotReady => unreachable!(),

            PromiseResult::Successful(_) => {
                log!("tokens returned {}", amount.0);
            }

            PromiseResult::Failed => {
                log!(
                    "token transfer failed {}. recovering account state",
                    amount.0
                );
                self.recover_state(&user, amount.0);
            }
        }
    }

    #[private]
    pub fn return_tokens_treasury_callback(&mut self, amount: U128) {
        match env::promise_result(0) {
            PromiseResult::NotReady => unreachable!(),

            PromiseResult::Successful(_) => {
                log!("tokens returned {}", amount.0);
            }

            PromiseResult::Failed => {
                log!(
                    "token transfer failed {}.",
                    amount.0
                );
            }
        }
    }

    fn recover_state(&mut self, user: &AccountId, staked: u128) {
        let mut v;
        if let Some(v2) = self.vaults.get(&user) {
            v = v2;
            v.staked += staked;
        } else {
            // If the vault was closed before by another TX, then we must recover the state
            self.accounts_registered += 1;
            v = Vault { staked }
        }

        self.vaults.insert(user, &v);
    }

    fn assert_owner(&self) {
        assert!(
            env::predecessor_account_id() == self.owner_id,
            "can only be called by the owner"
        );
    }

    fn is_contract_closed(&self) -> bool{
        Contract::get_epoch_millis() > self.closing_date
    }

    fn assert_not_closed(&self){
        assert!(
            !self.is_contract_closed(),
            "contract is closed"
        );
    }

    pub fn get_epoch_millis() -> u64{
        return env::block_timestamp() / SECOND;
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
#[allow(unused_imports)]
mod tests {
    use near_contract_standards::fungible_token::receiver::FungibleTokenReceiver;
    use near_contract_standards::storage_management::StorageManagement;
    use near_sdk::test_utils::{accounts, VMContextBuilder};
    use near_sdk::{testing_env, Balance};
    use near_sdk::{MockedBlockchain, ValidatorId};
    use std::convert::TryInto;

    use super::*;

    fn acc_staking() -> ValidAccountId {
        "test-token".try_into().unwrap()
    }

    fn acc_trasury() -> ValidAccountId {
        "treasury".try_into().unwrap()
    }

    fn acc_owner() -> ValidAccountId {
        "owner".try_into().unwrap()
    }

    /// deposit_dec = size of deposit in e24 to set for the next transacton
    fn setup_contract<F: Fn(&VMContextBuilder) -> u64>(
        predecessor: ValidAccountId,
        deposit_dec: u128,
        returnable: bool,
        closing_date: F
    ) -> (VMContextBuilder, Contract) {
        let mut context = VMContextBuilder::new();
        testing_env!(context.build());
        testing_env!(context
            .predecessor_account_id(predecessor)
            .attached_deposit((deposit_dec).into())
            .block_timestamp(100_000_000_000_000_000u64)
            .build());
        let contract = Contract::new(
            acc_owner(), // owner
            acc_staking(),
            acc_trasury(),
            returnable,
            closing_date(&context)
        );
        (context, contract)
    }

    fn stake(ctx: &mut VMContextBuilder, ctr: &mut Contract, a: &ValidAccountId, amount: u128) {
        testing_env!(ctx
            .attached_deposit(0)
            .predecessor_account_id(acc_staking())
            .block_timestamp(100_000_000_000_000_000u64)
            .build());
        ctr.ft_on_transfer(a.clone(), amount.into(), "transfer to pool".to_string());
    }
    fn unstake(ctx: &mut VMContextBuilder, ctr: &mut Contract, a: &ValidAccountId, amount: u128) {
        testing_env!(ctx
            .attached_deposit(1)
            .predecessor_account_id(a.clone())
            .block_timestamp(100_000_000_000_000_000u64)
            .build());
        ctr.unstake(amount.into());
    }
    fn withdraw_to_treasury(ctx: &mut VMContextBuilder, ctr: &mut Contract){
        testing_env!(ctx
            .attached_deposit(0)
            .block_timestamp(100_000_000_000_000_000u64)
            .build());
        ctr.withdraw_tokens();
    }

    fn get_next_year_epoch(ctx: &VMContextBuilder) -> u64{
        get_time_millis(ctx) + 31556952000u64
    }

    fn get_time_millis(ctx: &VMContextBuilder) -> u64{
        ctx.context.block_timestamp / SECOND
    }

    #[test]
    fn test_set_active() {
        let (_, mut ctr) = setup_contract(acc_owner(), 5, false, |ctx |get_next_year_epoch(&ctx));
        assert_eq!(ctr.is_active, true);
        ctr.set_active(false);
        assert_eq!(ctr.is_active, false);
    }

    #[test]
    #[should_panic(expected = "can only be called by the owner")]
    fn test_set_active_not_admin() {
        let (_, mut ctr) = setup_contract(accounts(1), 0, false, |ctx |get_next_year_epoch(&ctx));
        ctr.set_active(false);
    }

    #[test]
    fn test_set_closing_date() {
        let (_, mut ctr) = setup_contract(acc_owner(), 5, false, |ctx| 10_000);
        assert_eq!(ctr.closing_date, 10_000);
        ctr.set_closing_date(20_000);
        assert_eq!(ctr.closing_date, 20_000);
    }

    #[test]
    #[should_panic(expected = "can only be called by the owner")]
    fn test_set_closing_date_not_admin() {
        let (ctx, mut ctr) = setup_contract(accounts(1), 0, false, |ctx |get_next_year_epoch(&ctx));
        ctr.set_closing_date(get_next_year_epoch(&ctx) + 10_000);
    }

    #[test]
    #[should_panic(
        expected = "The attached deposit is less than the minimum storage balance (50000000000000000000000)"
    )]
    fn test_min_storage_deposit() {
        let (mut ctx, mut ctr) = setup_contract(accounts(0), 0, false, |ctx |get_next_year_epoch(&ctx));
        testing_env!(ctx.attached_deposit(NEAR_BALANCE / 4).build());
        ctr.storage_deposit(None, None);
    }

    #[test]
    fn test_storage_deposit() {
        let user = accounts(1);
        let (mut ctx, mut ctr) = setup_contract(user.clone(), 0, false, |ctx |get_next_year_epoch(&ctx));

        match ctr.storage_balance_of(user.clone()) {
            Some(_) => panic!("unregistered account must not have a balance"),
            _ => {}
        };

        testing_env!(ctx.attached_deposit(NEAR_BALANCE).build());
        ctr.storage_deposit(None, None);
        match ctr.storage_balance_of(user) {
            None => panic!("user account should be registered"),
            Some(s) => {
                assert_eq!(s.available.0, 0, "availabe should be 0");
                assert_eq!(
                    s.total.0, NEAR_BALANCE,
                    "total user storage deposit should be correct"
                );
            }
        }
    }

    #[test]
    #[should_panic(expected = "contract is not active")]
    fn test_storage_deposit_inactive() {
        let user = accounts(1);
        let (mut ctx, mut ctr) = setup_contract(acc_owner(), 0, false, |ctx |get_next_year_epoch(&ctx));
        ctr.set_active(false);
        testing_env!(ctx.predecessor_account_id(user.clone()).build());
        testing_env!(ctx.attached_deposit(NEAR_BALANCE).build());
        ctr.storage_deposit(None, None);
    }

    #[test]
    #[should_panic(expected = "contract is closed")]
    fn test_storage_deposit_closed() {
        let user = accounts(1);
        let (mut ctx, mut ctr) = setup_contract(user.clone(), 0, false, |ctx |get_time_millis(&ctx) - 1_000);

        testing_env!(ctx.attached_deposit(NEAR_BALANCE).build());
        ctr.storage_deposit(None, None);
    }

    #[test]
    fn test_staking() {
        let user = accounts(1);
        let (mut ctx, mut ctr) = setup_contract(user.clone(), 0, false, |ctx |get_next_year_epoch(&ctx));

        // register an account
        testing_env!(ctx.attached_deposit(NEAR_BALANCE).build());
        ctr.storage_deposit(None, None);

        // ------------------------------------------------
        // check correct user stacked
        stake(&mut ctx, &mut ctr, &user, E24*1_000);
        let staked_0 = ctr.status(get_acc(2));
        assert_eq!(staked_0.0, 0, "account2 didn't stake");

        // correct stake
        let staked_1 = ctr.status(user.clone().into());
        assert_eq!(staked_1.0, E24*1_000, "incorrect staked status for user");
    }

    #[test]
    #[should_panic(expected = "staked amount must be positive")]
    fn test_staking_nothing() {
        let user = accounts(1);
        let (mut ctx, mut ctr) = setup_contract(user.clone(), 0, false, |ctx |get_next_year_epoch(&ctx));

        // register an account
        testing_env!(ctx.attached_deposit(NEAR_BALANCE).build());
        ctr.storage_deposit(None, None);

        // ------------------------------------------------
        // stake 0
        stake(&mut ctx, &mut ctr, &user, 0);
    }
    
    #[test]
    #[should_panic(expected = "Only test-token token transfers are accepted")]
    fn test_staking_wrong_token() {
        let user = accounts(1);
        let (mut ctx, mut ctr) = setup_contract(user.clone(), 0, false, |ctx |get_next_year_epoch(&ctx));

        // register an account
        testing_env!(ctx.attached_deposit(NEAR_BALANCE).build());
        ctr.storage_deposit(None, None);

        // ------------------------------------------------
        // set predecesor acc as wrong token and try stake
        testing_env!(ctx
            .attached_deposit(0)
            .predecessor_account_id(accounts(4))
            .block_timestamp(100_000_000_000_000_000u64)
            .build());
        ctr.ft_on_transfer(user.clone(), (E24*1_000).into(), "transfer to pool".to_string());
    }

    #[test]
    #[should_panic(expected = "contract is not active")]
    fn test_stacking_ctr_not_active() {
        let user = accounts(1);
        let (mut ctx, mut ctr) = setup_contract(acc_owner(), 0, false, |ctx |get_next_year_epoch(&ctx));

        ctr.set_active(false);
        testing_env!(ctx.predecessor_account_id(user.clone()).build());

        // register an account
        testing_env!(ctx.attached_deposit(NEAR_BALANCE).build());
        ctr.storage_deposit(None, None);

        // ------------------------------------------------
        // stake when contract is not active       
        stake(&mut ctx, &mut ctr, &user, E24*1_000);
    }
    #[test]
    #[should_panic(expected = "contract is closed")]
    fn test_stacking_ctr_closed() {
        let user = accounts(1);
        let (mut ctx, mut ctr) = setup_contract(user.clone(), 0, false,
         |ctx| get_time_millis(&ctx) - 10_000);

        // register an account
        testing_env!(ctx.attached_deposit(NEAR_BALANCE).build());
        ctr.storage_deposit(None, None);

        // ------------------------------------------------
        // stake when contract is closed       
        stake(&mut ctx, &mut ctr, &user, E24*1_000);
    }

    #[test]
    #[should_panic(expected = "E10: account not found. Register the account.")]
    fn test_stake_inexistent_account() {
        let user = accounts(1);
        let (mut ctx, mut ctr) = setup_contract(user.clone(), 0, false, |ctx |get_next_year_epoch(&ctx));

        // ------------------------------------------------
        // stake
        stake(&mut ctx, &mut ctr, &user, E24);
    }

    #[test]
    fn test_unstake() {
        let user = accounts(1);
        let (mut ctx, mut ctr) = setup_contract(user.clone(), 0, false, |ctx |get_next_year_epoch(&ctx));

        // register an account
        testing_env!(ctx.attached_deposit(NEAR_BALANCE).build());
        ctr.storage_deposit(None, None);

        // stake    
        stake(&mut ctx, &mut ctr, &user, E24*2_000);

        // ------------------------------------------------
        // correct unstake
        unstake(&mut ctx, &mut ctr, &user, E24*1_000);
        let unstaked_0 = ctr.status(user.clone().into());
        assert_eq!(unstaked_0.0, E24*1_000, "wrong user stake balance");
    }

    #[test]
    #[should_panic(expected = "Requires attached deposit of exactly 1 yoctoNEAR")]
    fn test_unstake_no_deposit() {
        let user = accounts(1);
        let (mut ctx, mut ctr) = setup_contract(user.clone(), 0, false, |ctx |get_next_year_epoch(&ctx));

        // register an account
        testing_env!(ctx.attached_deposit(NEAR_BALANCE).build());
        ctr.storage_deposit(None, None);

        // stake    
        stake(&mut ctx, &mut ctr, &user, E24*2_000);

        // ------------------------------------------------
        // unstake with no deposit (yoctonear)
        testing_env!(ctx
            .attached_deposit(0)
            .block_timestamp(100_000_000_000_000_000u64)
            .build());
        ctr.unstake((E24*1_000).into());
    }

    #[test]
    #[should_panic(expected = "E30: not enough staked tokens")]
    fn test_unstake_more_than_balance() {
        let user = accounts(1);
        let (mut ctx, mut ctr) = setup_contract(user.clone(), 0, false, |ctx |get_next_year_epoch(&ctx));

        // register an account
        testing_env!(ctx.attached_deposit(NEAR_BALANCE).build());
        ctr.storage_deposit(None, None);

        // stake    
        stake(&mut ctx, &mut ctr, &user, E24*1_000);

        // ------------------------------------------------
        // unstake more than staked
        unstake(&mut ctx, &mut ctr, &user, E24*1_001);
    }

    #[test]
    fn test_unstake_all() {
        let user = accounts(1);
        let (mut ctx, mut ctr) = setup_contract(user.clone(), 0, false, |ctx |get_next_year_epoch(&ctx));

        // register an account
        testing_env!(ctx.attached_deposit(NEAR_BALANCE).build());
        ctr.storage_deposit(None, None);

        // stake    
        stake(&mut ctx, &mut ctr, &user, E24*1_000);

        // ------------------------------------------------
        // unstake all
        unstake(&mut ctx, &mut ctr, &user, E24*1_000);

        let unstaked_0 = ctr.storage_balance_of(user.clone().into());
        if let None = unstaked_0 {
            return;
        }    
        panic!("user vault should have been removed");
    }

    #[test]
    #[should_panic(expected = "contract is not active")]
    fn test_unstake_when_not_active() {
        let user = accounts(1);
        let (mut ctx, mut ctr) = setup_contract(user.clone(), 0, false, |ctx |get_next_year_epoch(&ctx));

        // register an account
        testing_env!(ctx.attached_deposit(NEAR_BALANCE).build());
        ctr.storage_deposit(None, None);

        // stake    
        stake(&mut ctx, &mut ctr, &user, E24*1_000);

        testing_env!(ctx.predecessor_account_id(acc_owner()).build());
        ctr.set_active(false);
        testing_env!(ctx.predecessor_account_id(user.clone()).build());

        // ------------------------------------------------
        // unstake
        unstake(&mut ctx, &mut ctr, &user, E24*1_000);
    }

    #[test]
    #[should_panic(expected = "contract is closed")]
    fn test_unstake_when_closed_not_returnable() {
        let user = accounts(1);
        let (mut ctx, mut ctr) = setup_contract(user.clone(), 0, false, |ctx |get_next_year_epoch(&ctx));

        // register an account
        testing_env!(ctx.attached_deposit(NEAR_BALANCE).build());
        ctr.storage_deposit(None, None);

        // stake    
        stake(&mut ctx, &mut ctr, &user, E24*1_000);

        testing_env!(ctx.predecessor_account_id(acc_owner()).build());
        // set closing date in the past
        ctr.set_closing_date(get_time_millis(&ctx) - 10_000);
        testing_env!(ctx.predecessor_account_id(user.clone()).build());

        // ------------------------------------------------
        // unstake
        unstake(&mut ctx, &mut ctr, &user, E24*1_000);
    }

    #[test]
    fn test_unstake_when_closed_returnable() {
        let user = accounts(1);
        let (mut ctx, mut ctr) = setup_contract(user.clone(), 0, true, |ctx |get_next_year_epoch(&ctx));

        // register an account
        testing_env!(ctx.attached_deposit(NEAR_BALANCE).build());
        ctr.storage_deposit(None, None);

        // stake    
        stake(&mut ctx, &mut ctr, &user, E24*2_000);

        testing_env!(ctx.predecessor_account_id(acc_owner()).build());
        // set closing date in the past
        ctr.set_closing_date(get_time_millis(&ctx) - 10_000);
        testing_env!(ctx.predecessor_account_id(user.clone()).build());

        // ------------------------------------------------
        // unstake
        unstake(&mut ctx, &mut ctr, &user, E24*1_000);
        let unstaked_0 = ctr.status(user.clone().into());
        assert_eq!(unstaked_0.0, E24*1_000, "wrong user stake balance");
    }

    #[test]
    #[should_panic(expected = "E10: account not found. Register the account.")]
    fn test_unstake_inexistent_account() {
        let user = accounts(1);
        let (mut ctx, mut ctr) = setup_contract(user.clone(), 0, false, |ctx |get_next_year_epoch(&ctx));

        // ------------------------------------------------
        // unstake
        unstake(&mut ctx, &mut ctr, &user, E24);
    }

    #[test]
    #[should_panic(expected = "this tokens are returnable")]
    fn test_withdraw_to_treasury_when_returnable(){
        let user = accounts(1);
        let user_2 = accounts(2);
        let (mut ctx, mut ctr) = setup_contract(user.clone(), 0, true, |ctx |get_next_year_epoch(&ctx));
        
        // register an account
        testing_env!(ctx.attached_deposit(NEAR_BALANCE).build());
        ctr.storage_deposit(None, None);

        // stake    
        stake(&mut ctx, &mut ctr, &user, E24*1_000);

        // register another account
        testing_env!(ctx.predecessor_account_id(user_2.clone()).build());
        testing_env!(ctx.attached_deposit(NEAR_BALANCE).build());
        ctr.storage_deposit(None, None);

        // stake    
        stake(&mut ctx, &mut ctr, &user_2, E24*1_000);

        testing_env!(ctx.predecessor_account_id(acc_owner()).build());

        // withdraw all tokens to treasury account
        withdraw_to_treasury(&mut ctx, &mut ctr);
    }

    #[test]
    #[should_panic(expected = "can only be called by the owner")]
    fn test_withdraw_to_treasury_not_owner(){
        let user = accounts(1);
        let (mut ctx, mut ctr) = setup_contract(user.clone(), 0, false, |ctx| get_time_millis(&ctx) - 10_000);

        // withdraw all tokens to treasury account
        withdraw_to_treasury(&mut ctx, &mut ctr);
    }

    #[test]
    #[should_panic(expected = "contract is not closed")]
    fn test_withdraw_to_treasury_not_closed(){
        let user = accounts(1);
        let user_2 = accounts(2);
        let (mut ctx, mut ctr) = setup_contract(user.clone(), 0, false, |ctx |get_next_year_epoch(&ctx));
        
        // register an account
        testing_env!(ctx.attached_deposit(NEAR_BALANCE).build());
        ctr.storage_deposit(None, None);

        // stake    
        stake(&mut ctx, &mut ctr, &user, E24*1_000);

        // register another account
        testing_env!(ctx.predecessor_account_id(user_2.clone()).build());
        testing_env!(ctx.attached_deposit(NEAR_BALANCE).build());
        ctr.storage_deposit(None, None);

        // stake    
        stake(&mut ctx, &mut ctr, &user_2, E24*1_000);

        testing_env!(ctx.predecessor_account_id(acc_owner()).build());

        // withdraw all tokens to treasury account
        withdraw_to_treasury(&mut ctx, &mut ctr);
    }

    #[test]
    fn test_withdraw_to_treasury(){
        let user = accounts(1);
        let user_2 = accounts(2);
        let (mut ctx, mut ctr) = setup_contract(user.clone(), 0, false, |ctx |get_next_year_epoch(&ctx));
        
        // register an account
        testing_env!(ctx.attached_deposit(NEAR_BALANCE).build());
        ctr.storage_deposit(None, None);

        // stake    
        stake(&mut ctx, &mut ctr, &user, E24*1_000);

        // register another account
        testing_env!(ctx.predecessor_account_id(user_2.clone()).build());
        testing_env!(ctx.attached_deposit(NEAR_BALANCE).build());
        ctr.storage_deposit(None, None);

        // stake    
        stake(&mut ctx, &mut ctr, &user_2, E24*1_000);

        testing_env!(ctx.predecessor_account_id(acc_owner()).build());
        ctr.set_closing_date(get_time_millis(&ctx) - 10_000);

        // withdraw all tokens to treasury account
        withdraw_to_treasury(&mut ctx, &mut ctr);
    }

    #[test]
    #[should_panic(expected = "Storage withdraw not possible, close the account instead")]
    fn test_storage_withdraw(){
        let user = accounts(1);
        let (mut ctx, mut ctr) = setup_contract(user.clone(), 0, false, |ctx |get_next_year_epoch(&ctx));

        // register an account
        testing_env!(ctx.attached_deposit(NEAR_BALANCE).build());
        ctr.storage_deposit(None, None);

        // stake    
        stake(&mut ctx, &mut ctr, &user, E24*2_000);

        // ------------------------------------------------
        // try withdraw
        testing_env!(ctx
            .attached_deposit(0)
            .block_timestamp(100_000_000_000_000_000u64)
            .build());
        ctr.storage_withdraw(Some((E24*1_000).into()));
    }

    #[test]
    fn test_storage_unregister_force(){
        let user = accounts(1);
        let (mut ctx, mut ctr) = setup_contract(user.clone(), 0, true, |ctx |get_next_year_epoch(&ctx));

        // register an account
        testing_env!(ctx.attached_deposit(NEAR_BALANCE).build());
        ctr.storage_deposit(None, None);

        // stake    
        stake(&mut ctx, &mut ctr, &user, E24*2_000);

        // ------------------------------------------------
        // try unregister
        testing_env!(ctx
            .attached_deposit(1)
            .predecessor_account_id(user.clone())
            .block_timestamp(100_000_000_000_000_000u64)
            .build());
        ctr.storage_unregister(Some(true));

        let unstaked_0 = ctr.storage_balance_of(user.clone().into());
        if let None = unstaked_0 {
            return;
        }    
        panic!("user vault should have been removed");
    }

    #[test]
    #[should_panic(expected = "Requires attached deposit of exactly 1 yoctoNEAR")]
    fn test_storage_unregister_force_no_deposit(){
        let user = accounts(1);
        let (mut ctx, mut ctr) = setup_contract(user.clone(), 0, true, |ctx |get_next_year_epoch(&ctx));

        // register an account
        testing_env!(ctx.attached_deposit(NEAR_BALANCE).build());
        ctr.storage_deposit(None, None);

        // stake    
        stake(&mut ctx, &mut ctr, &user, E24*2_000);

        // ------------------------------------------------
        // try unregister
        testing_env!(ctx
            .attached_deposit(0)
            .block_timestamp(100_000_000_000_000_000u64)
            .build());
        ctr.storage_unregister(Some(true));
    }

    #[test]
    #[should_panic(expected = "contract is not active")]
    fn test_storage_unregister_not_active(){
        let user = accounts(1);
        let (mut ctx, mut ctr) = setup_contract(user.clone(), 0, true, |ctx |get_next_year_epoch(&ctx));

        // register an account
        testing_env!(ctx.attached_deposit(NEAR_BALANCE).build());
        ctr.storage_deposit(None, None);

        // stake    
        stake(&mut ctx, &mut ctr, &user, E24*2_000);

        testing_env!(ctx.predecessor_account_id(acc_owner()).build());
        ctr.set_active(false);
        // ------------------------------------------------
        // try unregister
        testing_env!(ctx
            .attached_deposit(1)
            .predecessor_account_id(user.clone())
            .block_timestamp(100_000_000_000_000_000u64)
            .build());
        ctr.storage_unregister(Some(true));
    }

    #[test]
    #[should_panic(expected = "contract is closed")]
    fn test_storage_unregister_closed_not_returnable(){
        let user = accounts(1);
        let (mut ctx, mut ctr) = setup_contract(user.clone(), 0, false, |ctx |get_next_year_epoch(&ctx));

        // register an account
        testing_env!(ctx.attached_deposit(NEAR_BALANCE).build());
        ctr.storage_deposit(None, None);

        // stake    
        stake(&mut ctx, &mut ctr, &user, E24*2_000);

        testing_env!(ctx.predecessor_account_id(acc_owner()).build());
        ctr.set_closing_date(get_time_millis(&ctx) - 10_000);
        // ------------------------------------------------
        // try unregister
        testing_env!(ctx
            .attached_deposit(1)
            .predecessor_account_id(user.clone())
            .block_timestamp(100_000_000_000_000_000u64)
            .build());
        ctr.storage_unregister(Some(true));
    }

    #[test]
    fn test_storage_unregister_closed_returnable(){
        let user = accounts(1);
        let (mut ctx, mut ctr) = setup_contract(user.clone(), 0, true, |ctx |get_next_year_epoch(&ctx));

        // register an account
        testing_env!(ctx.attached_deposit(NEAR_BALANCE).build());
        ctr.storage_deposit(None, None);

        // stake    
        stake(&mut ctx, &mut ctr, &user, E24*2_000);

        testing_env!(ctx.predecessor_account_id(acc_owner()).build());
        ctr.set_closing_date(get_time_millis(&ctx) - 10_000);
        // ------------------------------------------------
        // try unregister
        testing_env!(ctx
            .attached_deposit(1)
            .predecessor_account_id(user.clone())
            .block_timestamp(100_000_000_000_000_000u64)
            .build());
        ctr.storage_unregister(Some(true));

        let unstaked_0 = ctr.storage_balance_of(user.clone().into());
        if let None = unstaked_0 {
            return;
        }    
        panic!("user vault should have been removed");
    }

    #[test]
    fn test_close(){
        let user = accounts(1);
        let (mut ctx, mut ctr) = setup_contract(user.clone(), 0, true, |ctx |get_next_year_epoch(&ctx));

        // register an account
        testing_env!(ctx.attached_deposit(NEAR_BALANCE).build());
        ctr.storage_deposit(None, None);

        // stake    
        stake(&mut ctx, &mut ctr, &user, E24*2_000);

        // ------------------------------------------------
        // try to close
        testing_env!(ctx
            .attached_deposit(1)
            .predecessor_account_id(user.clone())
            .block_timestamp(100_000_000_000_000_000u64)
            .build());
        ctr.close();

        let unstaked_0 = ctr.storage_balance_of(user.clone().into());
        if let None = unstaked_0 {
            return;
        }    
        panic!("user vault should have been removed");
    }

    #[test]
    #[should_panic(expected = "Requires attached deposit of exactly 1 yoctoNEAR")]
    fn test_close_no_deposit(){
        let user = accounts(1);
        let (mut ctx, mut ctr) = setup_contract(user.clone(), 0, true, |ctx |get_next_year_epoch(&ctx));

        // register an account
        testing_env!(ctx.attached_deposit(NEAR_BALANCE).build());
        ctr.storage_deposit(None, None);

        // stake    
        stake(&mut ctx, &mut ctr, &user, E24*2_000);

        // ------------------------------------------------
        // try to close
        testing_env!(ctx
            .attached_deposit(0)
            .block_timestamp(100_000_000_000_000_000u64)
            .build());
        ctr.close();
    }

    #[test]
    #[should_panic(expected = "contract is not active")]
    fn test_close_not_active(){
        let user = accounts(1);
        let (mut ctx, mut ctr) = setup_contract(user.clone(), 0, true, |ctx |get_next_year_epoch(&ctx));

        // register an account
        testing_env!(ctx.attached_deposit(NEAR_BALANCE).build());
        ctr.storage_deposit(None, None);

        // stake    
        stake(&mut ctx, &mut ctr, &user, E24*2_000);

        testing_env!(ctx.predecessor_account_id(acc_owner()).build());
        ctr.set_active(false);
        // ------------------------------------------------
        // try to close
        testing_env!(ctx
            .attached_deposit(1)
            .predecessor_account_id(user.clone())
            .block_timestamp(100_000_000_000_000_000u64)
            .build());
        ctr.close();
    }

    #[test]
    #[should_panic(expected = "contract is closed")]
    fn test_close_ctr_closed_not_returnable(){
        let user = accounts(1);
        let (mut ctx, mut ctr) = setup_contract(user.clone(), 0, false, |ctx |get_next_year_epoch(&ctx));

        // register an account
        testing_env!(ctx.attached_deposit(NEAR_BALANCE).build());
        ctr.storage_deposit(None, None);

        // stake    
        stake(&mut ctx, &mut ctr, &user, E24*2_000);

        testing_env!(ctx.predecessor_account_id(acc_owner()).build());
        ctr.set_closing_date(get_time_millis(&ctx) - 10_000);
        // ------------------------------------------------
        // try to close
        testing_env!(ctx
            .attached_deposit(1)
            .predecessor_account_id(user.clone())
            .block_timestamp(100_000_000_000_000_000u64)
            .build());
        ctr.close();
    }

    #[test]
    #[should_panic(expected = "E10: account not found. Register the account.")]
    fn test_close_inexistent_account(){
        let user = accounts(1);
        let (mut ctx, mut ctr) = setup_contract(user.clone(), 0, true, |ctx |get_next_year_epoch(&ctx));

        // ------------------------------------------------
        // try to close
        testing_env!(ctx
            .attached_deposit(1)
            .predecessor_account_id(user.clone())
            .block_timestamp(100_000_000_000_000_000u64)
            .build());
        ctr.close();
    }

    fn get_acc(idx: usize) -> AccountId {
        accounts(idx).as_ref().to_string()
    }
}
