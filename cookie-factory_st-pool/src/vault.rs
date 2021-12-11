//! Vault is information per user about their balances in the exchange.

use near_sdk::borsh::{self, BorshDeserialize, BorshSerialize};
use near_sdk::json_types::{ValidAccountId, U128};
use near_sdk::{env, log, AccountId, Balance, PromiseOrValue};

use near_contract_standards::fungible_token::receiver::FungibleTokenReceiver;
use near_contract_standards::storage_management::{
    StorageBalance, StorageBalanceBounds, StorageManagement,
};

// use crate::constants::*;
// use crate::errors::*;
// use crate::util::*;
use crate::*;

#[derive(BorshSerialize, BorshDeserialize)]
#[cfg_attr(feature = "test", derive(Default, Clone))]
pub struct Vault {
    /// amount of staking token locked in this vault
    pub staked: Balance
}

impl Contract {
    #[inline]
    pub(crate) fn get_vault(&self, account_id: &AccountId) -> Vault {
        self.vaults.get(account_id).expect(ERR10_NO_ACCOUNT)
    }
}

// token deposits are done through NEP-141 ft_transfer_call to the NEARswap contract.
#[near_bindgen]
impl FungibleTokenReceiver for Contract {
    /**
    FungibleTokenReceiver implementation
    Callback on receiving tokens by this contract.
    Automatically stakes receiving tokens.
    Returns zero.
    Panics when account is not registered or when receiving a wrong token. */
    #[allow(unused_variables)]
    fn ft_on_transfer(
        &mut self,
        sender_id: ValidAccountId,
        amount: U128,
        msg: String,
    ) -> PromiseOrValue<U128> {
        self.assert_is_active();
        self.assert_not_closed();
        let token = env::predecessor_account_id();
        assert!(
            token == self.staking_token,
            "Only {} token transfers are accepted",
            self.staking_token
        );
        assert!(amount.0 > 0, "staked amount must be positive");
        let sender_id: &AccountId = sender_id.as_ref();
        let mut v = self.get_vault(sender_id);

        log!("Staked, {} {}", amount.0, token);
        v.staked += amount.0;
        self.vaults.insert(sender_id, &v);
        self.total += amount.0;

        return PromiseOrValue::Value(U128(0));
    }
}

#[near_bindgen]
impl StorageManagement for Contract {
    /// Registers a new account
    #[allow(unused_variables)]
    #[payable]
    fn storage_deposit(
        &mut self,
        account_id: Option<ValidAccountId>,
        registration_only: Option<bool>,
    ) -> StorageBalance {
        self.assert_is_active();
        self.assert_not_closed();
        let amount: Balance = env::attached_deposit();
        let account_id = account_id
            .map(|a| a.into())
            .unwrap_or_else(|| env::predecessor_account_id());
        if let Some(_) = self.vaults.get(&account_id) {
            log!("The account is already registered, refunding the deposit");
            if amount > 0 {
                Promise::new(env::predecessor_account_id()).transfer(amount);
            }
        } else {
            assert!(
                amount >= NEAR_BALANCE,
                "The attached deposit is less than the minimum storage balance ({})",
                NEAR_BALANCE
            );
            self.create_account(&account_id, 0);

            let refund = amount - NEAR_BALANCE;
            if refund > 0 {
                Promise::new(env::predecessor_account_id()).transfer(refund);
            }
        }
        storage_balance()
    }

    /// Close the account (`close()` or `storage_unregister(true)`) to close the account and
    /// withdraw deposited NEAR.
    #[allow(unused_variables)]
    fn storage_withdraw(&mut self, amount: Option<U128>) -> StorageBalance {
        panic!("Storage withdraw not possible, close the account instead");
    }

    /// When force == true it will close the account. Otherwise this is noop.
    fn storage_unregister(&mut self, force: Option<bool>) -> bool {
        self.assert_is_active();
        if self.returnable == false {
            self.assert_not_closed();
        }
        if Some(true) == force {
            self.close();
            return true;
        }
        false
    }

    /// Mix and min balance is always MIN_BALANCE.
    fn storage_balance_bounds(&self) -> StorageBalanceBounds {
        StorageBalanceBounds {
            min: NEAR_BALANCE.into(),
            max: Some(NEAR_BALANCE.into()),
        }
    }

    /// If the account is registered the total and available balance is always MIN_BALANCE.
    /// Otherwise None.
    fn storage_balance_of(&self, account_id: ValidAccountId) -> Option<StorageBalance> {
        let account_id: AccountId = account_id.into();
        if let Some(_) = self.vaults.get(&account_id) {
            return Some(storage_balance());
        }
        None
    }
}

fn storage_balance() -> StorageBalance {
    StorageBalance {
        total: NEAR_BALANCE.into(),
        available: U128::from(0),
    }
}
