# Token Farm with Fixed Supply

The cookiefactory st-pool is the stacking pool for the cookiefactory NFT dapp

## Flow

Let's define a common variables:
```sh
# address of the pool
POOL=p2-pool.cookie-factory.testnet
# the token address we stake
STAKEING_TOKEN=abc.testnet
```

1. Register to the farm:
   ```
   near call $POOL storage_deposit '{}' --accountId me.testnet --deposit 0.05
   ```

2. Stake tokens:
   ```
   near call $STAKEING_TOKEN ft_transfer_call '{"receiver_id": ${POOL}, "amount":"10", "msg": "to farm"}' --accountId me.testnet --depositYocto 1 --gas=200000000000000
   ```

3. Enjoy stacking, stake more, and observe your status:
   ```
   near view $POOL status '{"account_id": "me.testnet"}'
   ```
