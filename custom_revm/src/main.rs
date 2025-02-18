use std::sync::Arc;

use revm::{
    primitives::{address, Address, Bytes, PrecompileResult, SpecId},
    ContextPrecompile, ContextStatefulPrecompile, ContextStatefulPrecompileArc, Database, Evm,
    InnerEvmContext,
};

#[allow(dead_code)]
const PRECOMPILE_COUNT_ZERO_BYTES_ADDRESS: Address =
    address!("0000000000000000000000000000000000000101");

// A precompile that takes an address as its input, reads the associated
// contract bytecode, and returns the number of zero bytes in it.
#[allow(dead_code)]
struct PrecompileCountZeroBytes;

impl<DB: Database> ContextStatefulPrecompile<DB> for PrecompileCountZeroBytes {
    fn call(
        &self,
        input: &Bytes,
        _gas_price: u64,
        context: &mut InnerEvmContext<DB>,
    ) -> PrecompileResult {
        let address: Address = Address::from_slice(input);
        let Ok((bytecode, _is_cold)) = context.code(address) else {
            unreachable!("")
        };
        let zero_bytes_count = bytecode.bytes().iter().filter(|&&byte| byte == 0).count();
        Ok((10, Bytes::from(zero_bytes_count.to_be_bytes().to_vec())))
    }
}

// Create a new (modified) EVM instance.
#[allow(dead_code)]
fn create_evm<'a, DB: Database + Sync + Send + Clone + 'static>(db: DB) -> Evm<'a, (), DB> {
    Evm::builder()
        .with_db(db)
        .with_spec_id(SpecId::HOMESTEAD)
        .append_handler_register(|handler| {
            let precompiles = handler.pre_execution.load_precompiles();
            handler.pre_execution.load_precompiles = Arc::new(move || {
                let mut precompiles = precompiles.clone();
                precompiles.extend([(
                    PRECOMPILE_COUNT_ZERO_BYTES_ADDRESS,
                    ContextPrecompile::ContextStateful(
                        Arc::new(PrecompileCountZeroBytes) as ContextStatefulPrecompileArc<DB>
                    ),
                )]);
                precompiles
            });
        })
        .build()
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    println!("Hello, world!");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    use revm::db::{CacheDB, EmptyDB};
    use revm::primitives::{AccountInfo, Bytecode, Bytes, ExecutionResult, TransactTo, U256};
    use std::str::FromStr;

    async fn deploy_contract_and_call_precompile(
        bytecode: &str,
    ) -> anyhow::Result<ExecutionResult> {
        // let address = address!("1100000000000000000000000000000000000001");

        // init db
        let mut db = CacheDB::new(EmptyDB::default());

        // insert contract bytecode
        let bytecode = Bytecode::new_raw(Bytes::from_str(bytecode)?);
        let code_hash = bytecode.hash_slow();

        db.insert_account_info(
            PRECOMPILE_COUNT_ZERO_BYTES_ADDRESS,
            AccountInfo {
                balance: U256::ZERO,
                nonce: 0_u64,
                code: Some(bytecode),
                code_hash,
            },
        );

        // build evm with transaction to precompile
        let mut evm = create_evm(db)
            .modify()
            .modify_tx_env(|tx| {
                // tx.caller = Address::ZERO;
                tx.transact_to = TransactTo::Call(PRECOMPILE_COUNT_ZERO_BYTES_ADDRESS);
                tx.data = PRECOMPILE_COUNT_ZERO_BYTES_ADDRESS
                    .as_slice()
                    .to_vec()
                    .into();
                // tx.value = U256::from(0);
            })
            .build();

        let result_and_state = evm.transact()?;
        return Ok(result_and_state.result);
    }

    async fn test_call_precompile(bytecode: &str, expected: u64) {
        let result = deploy_contract_and_call_precompile(bytecode).await;
        assert!(result.is_ok(), "call to precompile failed: {:?}", result);

        let result = result.unwrap();
        assert!(result.is_success(), "exec failed: {:?}", result);
        assert!(result.output().is_some(), "no output: {:?}", result);

        let a = U256::from_be_slice(&result.output().unwrap());
        let b = U256::from(expected);
        assert_eq!(a, b);
    }

    #[tokio::test]
    async fn test_empty_bytecode() {
        test_call_precompile("", 0).await;
    }

    #[tokio::test]
    async fn test_one_zero() {
        test_call_precompile("00", 1).await;
    }

    #[tokio::test]
    async fn test_one_zero_some_nonzero() {
        test_call_precompile("12340078", 1).await;
    }

    #[tokio::test]
    async fn test_single_zero_byte() {
        test_call_precompile("10023004500670", 0).await;
    }

    #[tokio::test]
    async fn test_some_zero_some_nonzero() {
        test_call_precompile("0011002200330044", 4).await;
    }
}
