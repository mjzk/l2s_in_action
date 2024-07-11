# L2s in Action

With the arrival of [Reth 1.0](https://github.com/paradigmxyz/reth), we have a relatively stable and modular Ethereum full node implementation. Many Ethereum L1 and L2 tasks can be completed by extending or modifying Reth.

In this `L2s in Action` series, I will walk you through a series of from-scratch tasks from simple to complex to build your own personalized L1 or L2 infrastructure.

-------------

### Chapter 1: Custom REVM

In this chapter, I will create an EVM variant with a custom precompile, building on [revm](https://github.com/bluealloy/revm). This custom precompile takes an address as its input, then reads the account's contract bytecode and returns the number of zero bytes in it.

### Chapter 2: Custom L2 indexer

In this chapter, I will create a L2 indexer which indexes all hypothetical L2 batches into a local database. The indexer is based on [Reth's ExEx(Executable Extension) framework](https://github.com/paradigmxyz/reth/tree/main/crates/exex).