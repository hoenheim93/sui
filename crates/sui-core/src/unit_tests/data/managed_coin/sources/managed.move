// Copyright (c) Mysten Labs, Inc.
// SPDX-License-Identifier: Apache-2.0

/// Example coin with a trusted manager responsible for minting/burning (e.g., a stablecoin)
/// By convention, modules defining custom coin types use upper case names, in contrast to
/// ordinary modules, which use camel case.
module fungible_tokens::managed {
    use std::option;
    use sui::coin::{Self, Coin, TreasuryCap};
    use sui::transfer;
    use sui::object::{Self, UID};
    use sui::table_vec::{Self, TableVec};
    use sui::tx_context::{Self, TxContext};

    struct PublicRedEnvelope has key, store {
        id: UID,
        coins: TableVec<Coin<MANAGED>>,
    }

    /// Name of the coin. By convention, this type has the same name as its parent module
    /// and has no fields. The full type of the coin defined by this module will be `COIN<MANAGED>`.
    struct MANAGED has drop {}

    /// Register the managed currency to acquire its `TreasuryCap`. Because
    /// this is a module initializer, it ensures the currency only gets
    /// registered once.
    fun init(witness: MANAGED, ctx: &mut TxContext) {
        // Get a treasury cap for the coin and give it to the transaction sender
        let (treasury_cap, metadata) = coin::create_currency<MANAGED>(witness, 2, b"MANAGED", b"", b"", option::none(), ctx);
        transfer::public_freeze_object(metadata);
        transfer::public_transfer(treasury_cap, tx_context::sender(ctx));

        let red_envelopes = PublicRedEnvelope { id: object::new(ctx), coins: table_vec::empty(ctx) };
        transfer::share_object(red_envelopes)
    }

    /// Manager can mint new coins
    public entry fun mint(
        treasury_cap: &mut TreasuryCap<MANAGED>, amount: u64, recipient: address, ctx: &mut TxContext
    ) {
        coin::mint_and_transfer(treasury_cap, amount, recipient, ctx)
    }
    
    public entry fun add_to_envelope(
        red_envelopes: &mut PublicRedEnvelope, coin: Coin<MANAGED>,
    ) {
        table_vec::push_back(&mut red_envelopes.coins, coin)
    }

    public entry fun take_from_envelope(
        red_envelopes: &mut PublicRedEnvelope, ctx: &mut TxContext
    ) {
        let coin = table_vec::pop_back(&mut red_envelopes.coins);
        transfer::public_transfer(coin, tx_context::sender(ctx))
    }

    /// Manager can burn coins
    public entry fun burn(treasury_cap: &mut TreasuryCap<MANAGED>, coin: Coin<MANAGED>) {
        coin::burn(treasury_cap, coin);
    }

    #[test_only]
    /// Wrapper of module initializer for testing
    public fun test_init(ctx: &mut TxContext) {
        init(MANAGED {}, ctx)
    }
}
