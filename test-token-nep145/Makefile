include ../Makefile_common

export CTR_ACC=test-token.cookie-factory.testnet

deploy-token:
	@cd ..; near deploy --wasmFile ./res/test-token_nep145.wasm --accountId $(CTR_ACC)  --initFunction "new" --initArgs "{\"owner_id\": \"$NMASTER_ACC\"}"