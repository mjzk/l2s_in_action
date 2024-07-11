use alloy_sol_types::{sol, SolEventInterface};
use futures::Future;
use reth_exex::{ExExContext, ExExEvent};
use reth_node_api::FullNodeComponents;
use reth_node_ethereum::EthereumNode;
use reth_primitives::{address, Address, Log, SealedBlockWithSenders, TransactionSigned};
use reth_provider::Chain;
use reth_tracing::tracing::info;
use rusqlite::Connection;

sol! {
    interface MyChain {
        event CommitBatch(uint256 indexed batchIndex, bytes32 indexed batchHash);
    }
}
use crate::MyChain::{CommitBatch, MyChainEvents};

#[allow(dead_code)]
const MY_CHAIN_ADDRESS: Address = address!("6666666666666666666666666666666666666666");

fn create_tables(connection: &mut Connection) -> rusqlite::Result<()> {
    //NOTE index => idx (index is the reserved keyword in SQLite)
    connection.execute(
        r#"
            CREATE TABLE IF NOT EXISTS batches (
                idx  INTEGER PRIMARY KEY,
                hash   TEXT NOT NULL
            );
            "#,
        (),
    )?;
    info!("Initialized database tables");
    Ok(())
}

async fn mychain_indexer_exex<Node: FullNodeComponents>(
    mut ctx: ExExContext<Node>,
    connection: Connection,
) -> eyre::Result<()> {
    // Process all new chain state notifications
    while let Some(notification) = ctx.notifications.recv().await {
        if let Some(committed_chain) = notification.committed_chain() {
            let events = decode_chain_into_events(&committed_chain);

            let mut commits = 0;

            for (_, _, _, event) in events {
                match event {
                    // CommitBatch
                    MyChainEvents::CommitBatch(CommitBatch {
                        batchIndex,
                        batchHash,
                    }) => {
                        let inserted = connection.execute(
                            r#"
                                INSERT INTO batches (idx, hash)
                                VALUES (?, ?)
                                "#,
                            (batchIndex.to_string(), batchHash.to_string()),
                        )?;
                        commits += inserted;
                        info!(%commits, "Committed batches");
                    } /*
                      handle other your L2 events here...
                       */
                }
            }

            ctx.events
                .send(ExExEvent::FinishedHeight(committed_chain.tip().number))?;
        }
    }

    Ok(())
}

fn decode_chain_into_events(
    chain: &Chain,
) -> impl Iterator<
    Item = (
        &SealedBlockWithSenders,
        &TransactionSigned,
        &Log,
        MyChainEvents,
    ),
> {
    chain
        // Get all blocks and receipts
        .blocks_and_receipts()
        // Get all receipts
        .flat_map(|(block, receipts)| {
            block
                .body
                .iter()
                .zip(receipts.iter().flatten())
                .map(move |(tx, receipt)| (block, tx, receipt))
        })
        .flat_map(|(block, tx, receipt)| {
            receipt
                .logs
                .iter()
                .filter(|log| MY_CHAIN_ADDRESS == log.address)
                .map(move |log| (block, tx, log))
        })
        // Decode and filter events
        .filter_map(|(block, tx, log)| {
            MyChainEvents::decode_raw_log(log.topics(), &log.data.data, true)
                .ok()
                .map(|event| (block, tx, log, event))
        })
}

async fn exex_init<Node: FullNodeComponents>(
    ctx: ExExContext<Node>,
    mut connection: Connection,
) -> eyre::Result<impl Future<Output = eyre::Result<()>>> {
    create_tables(&mut connection)?;
    Ok(mychain_indexer_exex(ctx, connection))
}

fn main() -> eyre::Result<()> {
    reth::cli::Cli::parse_args().run(|builder, _| async move {
        let handle = builder
            .node(EthereumNode::default())
            .install_exex("MychainIndexer", |ctx| async move {
                let connection = Connection::open("mychain_indexer.db")?;
                exex_init(ctx, connection).await
            })
            .launch()
            .await?;

        handle.wait_for_node_exit().await
    })
}

#[cfg(test)]
mod tests {
    use std::pin::pin;

    use alloy_sol_types::SolEvent;
    use reth::revm::db::BundleState;
    use reth_exex_test_utils::{test_exex_context, PollOnce};
    use reth_primitives::{
        Address, Block, Header, Log, Receipt, Transaction, TransactionSigned, TxKind, TxLegacy,
        TxType, U256,
    };
    use reth_provider::{Chain, ExecutionOutcome};
    use reth_testing_utils::generators::sign_tx_with_random_key_pair;
    use rusqlite::Connection;

    use crate::{MyChain::CommitBatch, MY_CHAIN_ADDRESS};

    /// Given the address of a bridge contract and an event, construct a transaction signed with a
    /// random private key and a receipt for that transaction.
    fn construct_tx_and_receipt<E: SolEvent>(
        to: Address,
        event: E,
    ) -> eyre::Result<(TransactionSigned, Receipt)> {
        let tx = Transaction::Legacy(TxLegacy {
            to: TxKind::Call(to),
            ..Default::default()
        });
        let log = Log::new(
            to,
            event
                .encode_topics()
                .into_iter()
                .map(|topic| topic.0)
                .collect(),
            event.encode_data().into(),
        )
        .ok_or_else(|| eyre::eyre!("failed to encode event"))?;
        #[allow(clippy::needless_update)] // side-effect of optimism fields
        let receipt = Receipt {
            tx_type: TxType::Legacy,
            success: true,
            cumulative_gas_used: 0,
            logs: vec![log],
            ..Default::default()
        };
        Ok((
            sign_tx_with_random_key_pair(&mut rand::thread_rng(), tx),
            receipt,
        ))
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn test_exex_two_commit_batches() -> eyre::Result<()> {
        // Initialize the test Execution Extension context with all dependencies
        let (ctx, handle) = test_exex_context().await?;
        // Create a temporary database file, so we can access it later for assertions
        let db_file = tempfile::NamedTempFile::new()?;

        // Initialize the ExEx
        let mut exex = pin!(super::exex_init(ctx, Connection::open(&db_file)?).await?);

        // Construct event, transaction and receipt
        let event = CommitBatch {
            batchIndex: U256::from(1),
            batchHash: Default::default(),
        };
        let (tx1, tx1_receipt) = construct_tx_and_receipt(MY_CHAIN_ADDRESS, event)?;

        // Construct event, transaction and receipt
        let event = CommitBatch {
            batchIndex: U256::from(2),
            batchHash: Default::default(),
        };
        let (tx2, tx2_receipt) = construct_tx_and_receipt(MY_CHAIN_ADDRESS, event)?;

        // Construct a block
        let block = Block {
            header: Header::default(),
            body: vec![tx1, tx2],
            ..Default::default()
        }
        .seal_slow()
        .seal_with_senders()
        .ok_or_else(|| eyre::eyre!("failed to recover senders"))?;

        // Construct a chain
        let chain = Chain::new(
            vec![block.clone()],
            ExecutionOutcome::new(
                BundleState::default(),
                vec![tx1_receipt, tx2_receipt].into(),
                block.number,
                vec![block.requests.clone().unwrap_or_default()],
            ),
            None,
        );

        // Send a notification that the chain has been committed
        handle
            .send_notification_chain_committed(chain.clone())
            .await?;
        // Poll the ExEx once, it will process the notification that we just sent
        exex.poll_once().await?;

        let connection = Connection::open(&db_file)?;

        // Assert that the event was parsed correctly and inserted into the database
        let batches: Vec<(u64, String)> = connection
            .prepare(r#"SELECT idx, hash FROM batches"#)?
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
            .collect::<Result<Vec<_>, _>>()?;
        assert_eq!(batches.len(), 2);
        let default_hash =
            reth_primitives::revm_primitives::FixedBytes::<32>::default().to_string();
        assert_eq!(batches[0], (1, default_hash.clone()));
        assert_eq!(batches[1], (2, default_hash.clone()));

        Ok(())
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn test_exex_one_commit_batch() -> eyre::Result<()> {
        // Initialize the test Execution Extension context with all dependencies
        let (ctx, handle) = test_exex_context().await?;
        // Create a temporary database file, so we can access it later for assertions
        let db_file = tempfile::NamedTempFile::new()?;

        // Initialize the ExEx
        let mut exex = pin!(super::exex_init(ctx, Connection::open(&db_file)?).await?);

        // Construct event, transaction and receipt
        let event = CommitBatch {
            batchIndex: U256::from(1),
            batchHash: Default::default(),
        };
        let (tx1, tx1_receipt) = construct_tx_and_receipt(MY_CHAIN_ADDRESS, event)?;

        // Construct a block
        let block = Block {
            header: Header::default(),
            body: vec![tx1],
            ..Default::default()
        }
        .seal_slow()
        .seal_with_senders()
        .ok_or_else(|| eyre::eyre!("failed to recover senders"))?;

        // Construct a chain
        let chain = Chain::new(
            vec![block.clone()],
            ExecutionOutcome::new(
                BundleState::default(),
                vec![tx1_receipt].into(),
                block.number,
                vec![block.requests.clone().unwrap_or_default()],
            ),
            None,
        );

        // Send a notification that the chain has been committed
        handle
            .send_notification_chain_committed(chain.clone())
            .await?;
        // Poll the ExEx once, it will process the notification that we just sent
        exex.poll_once().await?;

        let connection = Connection::open(&db_file)?;

        // Assert that the event was parsed correctly and inserted into the database
        let batches: Vec<(u64, String)> = connection
            .prepare(r#"SELECT idx, hash FROM batches"#)?
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
            .collect::<Result<Vec<_>, _>>()?;
        assert_eq!(batches.len(), 1);
        Ok(())
    }
}
