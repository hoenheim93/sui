// Copyright (c) Mysten Labs, Inc.
// SPDX-License-Identifier: Apache-2.0

use crate::{TestCaseImpl, TestContext};
use async_trait::async_trait;
use jsonrpsee::rpc_params;
use move_core_types::language_storage::StructTag;
use serde_json::json;
use tokio::time::{sleep, Duration};
use std::str::FromStr;
use std::{collections::HashMap};
use sui_core::test_utils::compile_ft_package;
use sui_indexer::schema::packages::package_id;
use sui_json::SuiJsonValue;
use sui_json_rpc_types::ObjectChange;
use sui_json_rpc_types::SuiTransactionBlockEffectsAPI;
use sui_json_rpc_types::{Balance, SuiObjectDataOptions, SuiTransactionBlockResponseOptions};
use sui_types::base_types::{ObjectID, ObjectRef};
use sui_types::gas_coin::GAS;
use sui_types::messages::ExecuteTransactionRequestType;
use sui_types::object::Owner;
use test_utils::messages::make_staking_transaction_with_wallet_context;
use tracing::info;

pub struct CoinIndexTest;

#[async_trait]
impl TestCaseImpl for CoinIndexTest {
    fn name(&self) -> &'static str {
        "CoinIndex"
    }

    fn description(&self) -> &'static str {
        "Test executing coin index"
    }

    async fn run(&self, ctx: &mut TestContext) -> Result<(), anyhow::Error> {
        let account = ctx.get_wallet_address();
        let client = ctx.clone_fullnode_client();
        let rgp = ctx.get_reference_gas_price().await;

        ctx.get_sui_from_faucet(None).await;
        let Balance {
            coin_object_count: mut old_coin_object_count,
            total_balance: mut old_total_balance,
            ..
        } = client.coin_read_api().get_balance(account, None).await?;

        let txn = ctx.make_transactions(1).await.swap_remove(0);

        let response = client
            .quorum_driver()
            .execute_transaction_block(
                txn,
                SuiTransactionBlockResponseOptions::new()
                    .with_effects()
                    .with_balance_changes(),
                Some(ExecuteTransactionRequestType::WaitForLocalExecution),
            )
            .await?;

        let balance_change = response.balance_changes.unwrap();
        let owner_balance = balance_change
            .iter()
            .find(|b| b.owner == Owner::AddressOwner(account))
            .unwrap();
        let recipient_balance = balance_change
            .iter()
            .find(|b| b.owner != Owner::AddressOwner(account))
            .unwrap();
        let Balance {
            coin_object_count,
            total_balance,
            coin_type,
            ..
        } = client.coin_read_api().get_balance(account, None).await?;
        assert_eq!(coin_type, GAS::type_().to_string());

        assert_eq!(coin_object_count, old_coin_object_count);
        assert_eq!(
            total_balance,
            (old_total_balance as i128 + owner_balance.amount) as u128
        );
        old_coin_object_count = coin_object_count;
        old_total_balance = total_balance;

        let Balance {
            coin_object_count,
            total_balance,
            ..
        } = client
            .coin_read_api()
            .get_balance(recipient_balance.owner.get_owner_address().unwrap(), None)
            .await?;
        assert_eq!(coin_object_count, 1);
        assert!(recipient_balance.amount > 0);
        assert_eq!(total_balance, recipient_balance.amount as u128);

        // Staking
        let validator_addr = ctx
            .get_latest_sui_system_state()
            .await
            .active_validators
            .get(0)
            .unwrap()
            .sui_address;
        let txn =
            make_staking_transaction_with_wallet_context(ctx.get_wallet_mut(), validator_addr)
                .await;

        let response = client
            .quorum_driver()
            .execute_transaction_block(
                txn,
                SuiTransactionBlockResponseOptions::new()
                    .with_effects()
                    .with_balance_changes(),
                Some(ExecuteTransactionRequestType::WaitForLocalExecution),
            )
            .await?;

        let balance_change = &response.balance_changes.unwrap()[0];
        assert_eq!(balance_change.owner, Owner::AddressOwner(account));

        let Balance {
            coin_object_count,
            total_balance,
            ..
        } = client.coin_read_api().get_balance(account, None).await?;
        assert_eq!(coin_object_count, old_coin_object_count - 1); // an object is staked
        assert_eq!(
            total_balance,
            (old_total_balance as i128 + balance_change.amount) as u128,
            "total_balance: {}, old_total_balance: {}, sui_balance_change.amount: {}",
            total_balance,
            old_total_balance,
            balance_change.amount
        );
        old_coin_object_count = coin_object_count;
        old_total_balance = total_balance;

        let (package, cap, envelope) = publish_ft_package(ctx).await?;
        let Balance { total_balance, .. } =
            client.coin_read_api().get_balance(account, None).await?;
        old_total_balance = total_balance;

        info!(
            "token package published, package: {:?}, cap: {:?}",
            package, cap
        );
        info!("account: {}", account);
        let sui_type_str = "0x2::sui::SUI";
        let coin_type_str = format!("{}::managed::MANAGED", package.0);
        info!("coin type: {}", coin_type_str);

        // Now mint 1 MANAGED coin to account, balance 10000
        let args = vec![
            SuiJsonValue::from_object_id(cap.0),
            SuiJsonValue::new(json!("10000"))?,
            SuiJsonValue::new(json!(account))?,
        ];
        let txn = client
            .transaction_builder()
            .move_call(
                account,
                package.0,
                "managed".into(),
                "mint".into(),
                vec![],
                args,
                None,
                rgp * 2_000_000,
            )
            .await
            .unwrap();
        let response = ctx.sign_and_execute(txn, "mint managed coin to self").await;

        // println!("balance: {:?}", response.balance_changes);
        let balance_changes = &response.balance_changes.unwrap();
        println!("balances changes: {:?}", balance_changes);
        let sui_balance_change = balance_changes
            .iter()
            .find(|b| b.coin_type.to_string().contains("SUI"))
            .unwrap();
        let managed_balance_change = balance_changes
            .iter()
            .find(|b| b.coin_type.to_string().contains("MANAGED"))
            .unwrap();

        assert_eq!(sui_balance_change.owner, Owner::AddressOwner(account));
        assert_eq!(managed_balance_change.owner, Owner::AddressOwner(account));

        let Balance { total_balance, .. } =
            client.coin_read_api().get_balance(account, None).await?;
        assert_eq!(coin_object_count, old_coin_object_count);
        assert_eq!(
            total_balance,
            (old_total_balance as i128 + sui_balance_change.amount) as u128,
            "total_balance: {}, old_total_balance: {}, sui_balance_change.amount: {}",
            total_balance,
            old_total_balance,
            sui_balance_change.amount
        );
        old_coin_object_count = coin_object_count;

        let Balance {
            coin_object_count: managed_coin_object_count,
            total_balance: managed_total_balance,
            // Important: update coin_type_str here because the leading 0s are truncated!
            coin_type: coin_type_str,
            ..
        } = client
            .coin_read_api()
            .get_balance(account, Some(coin_type_str.clone()))
            .await?;
        assert_eq!(managed_coin_object_count, 1); // minted one object
        assert_eq!(
            managed_total_balance,
            10000, // mint amount
        );

        let balances = client.coin_read_api().get_all_balances(account).await?;
        println!("balances: {:?}", balances);
        // Comes with asc order.
        assert_eq!(
            balances,
            vec![
                Balance {
                    coin_type: sui_type_str.into(), // coin_type_str.into(),
                    coin_object_count: old_coin_object_count,
                    total_balance,
                    locked_balance: HashMap::new(),
                },
                Balance {
                    coin_type: coin_type_str.clone(),
                    coin_object_count: 1,
                    total_balance: 10000,
                    locked_balance: HashMap::new(),
                },
            ],
        );

        // Now mint another MANAGED coin to account, balance 1
        let args = vec![
            SuiJsonValue::from_object_id(cap.0),
            SuiJsonValue::new(json!("1"))?,
            SuiJsonValue::new(json!(account))?,
        ];
        let txn = client
            .transaction_builder()
            .move_call(
                account,
                package.0,
                "managed".into(),
                "mint".into(),
                vec![],
                args,
                None,
                rgp * 2_000_000,
            )
            .await
            .unwrap();
        let response = ctx.sign_and_execute(txn, "mint managed coin to self").await;
        assert!(response.status_ok().unwrap());
        let old_total_balance = client
            .coin_read_api()
            .get_balance(account, None)
            .await
            .unwrap()
            .total_balance;
        let managed_balance = client
            .coin_read_api()
            .get_balance(account, Some(coin_type_str.clone()))
            .await
            .unwrap();
        let managed_coins = client
            .coin_read_api()
            .get_coins(account, Some(coin_type_str.clone()), None, None)
            .await
            .unwrap()
            .data;
        assert_eq!(managed_balance.total_balance, 10000 + 1);
        assert_eq!(managed_balance.coin_object_count, 1 + 1);
        assert_eq!(managed_coins.len(), 1 + 1);
        let managed_old_total_balance = managed_balance.total_balance;
        let managed_old_total_count = managed_balance.coin_object_count;

        // Now put the balance 1 mamanged coin into the envelope
        let managed_coin_id = managed_coins
            .iter()
            .find(|c| c.balance == 1)
            .unwrap()
            .coin_object_id;

        // Now add 1 MANAGED coin to account, balance 10000
        let args = vec![
            SuiJsonValue::from_object_id(envelope.0),
            SuiJsonValue::from_object_id(managed_coin_id),
        ];
        let txn = client
            .transaction_builder()
            .move_call(
                account,
                package.0,
                "managed".into(),
                "add_to_envelope".into(),
                vec![],
                args,
                None,
                rgp * 2_000_000,
            )
            .await
            .unwrap();
        println!("add managed coin to envelope");
        let response = ctx
            .sign_and_execute(txn, "add managed coin to envelope")
            .await;
        println!("add managed coin to envelope response: {:?}", response);
        assert!(response.status_ok().unwrap());
        println!("balance changes: {:?}", response.balance_changes);
        let managed_coins = client
            .coin_read_api()
            .get_coins(account, Some(coin_type_str.clone()), None, None)
            .await
            .unwrap()
            .data;
        println!("managed coins: {:?}", managed_coins);

        let owned_objs = client
            .read_api()
            .get_owned_objects(account, None, None, None)
            .await.unwrap().data;
        println!("owned_objs: {:?}", owned_objs);

 
        let magic_object = client
            .read_api()
            .get_object_with_options(managed_coin_id, SuiObjectDataOptions::bcs_lossless())
            .await.unwrap().data;
        println!("magic_object: {:?}", magic_object);

        let managed_balance = client
            .coin_read_api()
            .get_balance(account, Some(coin_type_str.clone()))
            .await
            .unwrap();
        println!("balances: {:?}", managed_balance);
        assert_eq!(managed_balance.total_balance, 10000); // <-=-
        assert_eq!(managed_balance.coin_object_count, 1); // <

        // // let obj = response.effects.unwrap().gas_object().reference.object_id;
        // let mut objs = client
        //     .coin_read_api()
        //     .get_coins(account, None, None, None)
        //     .await?
        //     .data;
        // let primary_coin = objs.swap_remove(0);
        // let coin_to_merge = objs.swap_remove(0);

        // .move_call(
        //     *address,
        //     SUI_FRAMEWORK_ADDRESS.into(),
        //     COIN_MODULE_NAME.to_string(),
        //     "mint_and_transfer".into(),
        //     type_args![coin_name]?,
        //     call_args![treasury_cap, 100000, address]?,
        //     Some(gas.object_id),
        //     10_000_000.into(),
        //     None,
        // )
        Ok(())
    }
}

async fn publish_ft_package(
    ctx: &mut TestContext,
) -> Result<(ObjectRef, ObjectRef, ObjectRef), anyhow::Error> {
    let compiled_package = compile_ft_package();
    let all_module_bytes =
        compiled_package.get_package_base64(/* with_unpublished_deps */ false);
    let dependencies = compiled_package.get_dependency_original_package_ids();

    let params = rpc_params![
        ctx.get_wallet_address(),
        all_module_bytes,
        dependencies,
        None::<ObjectID>,
        // Doesn't need to be scaled by RGP since most of the cost is storage
        500_000_000.to_string()
    ];

    let data = ctx
        .build_transaction_remotely("unsafe_publish", params)
        .await?;
    let response = ctx.sign_and_execute(data, "publish ft package").await;
    let changes = response.object_changes.unwrap();
    info!("changes: {:?}", changes);
    let pkg = changes
        .iter()
        .find(|change| matches!(change, ObjectChange::Published { .. }))
        .unwrap()
        .object_ref();
    let treasury_cap = changes
        .iter()
        .find(|change| {
            matches!(change, ObjectChange::Created {
            owner: Owner::AddressOwner(_),
            object_type: StructTag {
                address,
                module,
                name,
                ..
            },
            ..
        } if name.as_str() == "TreasuryCap")
        })
        .unwrap()
        .object_ref();
    let envelope = changes
        .iter()
        .find(|change| {
            matches!(change, ObjectChange::Created {
            owner: Owner::Shared {..},
            object_type: StructTag {
                name,
                ..
            },
            ..
        } if name.as_str() == "PublicRedEnvelope")
        })
        .unwrap()
        .object_ref();
    Ok((pkg, treasury_cap, envelope))
}
