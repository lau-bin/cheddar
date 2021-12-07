include ../Makefile_common

export CTR_ACC=pool.cookie-factory.testnet

deploy-pool:
	@cd ..; near deploy --wasmFile ./res/cookie-factory_st-pool.wasm --accountId $(CTR_ACC)  --initFunction "new" --initArgs "{\"owner_id\": \"$NMASTER_ACC\", \"staked_token\": \"test-token.cookie-factory.testnet\", \"treasury\": \"treasury\": \"treasury.cookie-factory.testnet\", \"returnable\": \"false\""}"