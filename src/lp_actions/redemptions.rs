use std::str::FromStr;

use cosmwasm_std::{to_binary, Addr, BankMsg, Coin, DepsMut, Env, Response, StdError, Uint128};
use injective_cosmwasm::{
    privileged_action::{PositionTransferAction, PrivilegedAction},
    InjectiveMsgWrapper, InjectiveQuerier, InjectiveQueryWrapper, Position, SubaccountId,
};
use injective_math::FPDecimal;

use crate::{
    state::{
        ADMIN_FEE_POSITIONS, ADMIN_OWNED_SHARES, CONFIG, DENOM_DECIMALS, IS_FUND_CLOSED,
        LP_POSITIONS, LP_TOTAL_SUPPLY,
    },
    ContractError,
};

use super::{
    derivative_position_helpers::{
        apply_funding_to_position, get_vault_estimated_position_notional,
    },
    oracle_price::get_oracle_price,
};

const ONE_YEAR_IN_SECONDS: u64 = 365 * 24 * 60 * 60;

pub fn get_vault_redemption_response(
    deps: DepsMut<InjectiveQueryWrapper>,
    env: &Env,
    sender: &Addr,
    redeemer_subaccount_id: SubaccountId,
) -> Result<Response<InjectiveMsgWrapper>, ContractError> {
    let querier = InjectiveQuerier::new(&deps.querier);
    let config = CONFIG.load(deps.storage)?;

    let subaccount_id = config.subaccount_id;
    let quote_denom = config.quote_denom;

    let denom_decimals = DENOM_DECIMALS.load(deps.storage)?;
    let quote_decimals = denom_decimals.get(&quote_denom).unwrap();

    let deposit_quote_res = querier.query_subaccount_deposit(&subaccount_id, &quote_denom)?;
    let vault_quote_total_balance = deposit_quote_res.deposits.total_balance;

    if vault_quote_total_balance < FPDecimal::zero() {
        return Err(ContractError::Std(StdError::generic_err(
            "Vault quote deposits are negative",
        )));
    }

    let lp_total_supply = LP_TOTAL_SUPPLY.load(deps.storage)?;
    let mut lp_positions = LP_POSITIONS.load(deps.storage)?;
    let lp_position = lp_positions
        .get(sender)
        .ok_or(ContractError::Std(StdError::generic_err(
            "Redeemer LP position does not exist",
        )))?;

    if env.block.time
        <= lp_position
            .subscription_time
            .plus_seconds(ONE_YEAR_IN_SECONDS)
    {
        return Err(ContractError::Std(StdError::generic_err(
            "Redeemer LP position has not been updated",
        )));
    }

    let lp_shares_to_burn = lp_position.shares;

    let mut funds_to_return = vec![];

    let quote_withdrawal_amount = vault_quote_total_balance * lp_shares_to_burn / lp_total_supply;
    funds_to_return.append(&mut vec![Coin {
        denom: quote_denom,
        amount: quote_withdrawal_amount.into(),
    }]);

    let mut total_redemption_notional = quote_withdrawal_amount;

    for (index, market_id) in config.spot_market_ids.iter().enumerate() {
        let market_res = querier.query_spot_market(market_id)?;
        let market = market_res.market.expect("market should be available");

        let deposit_base_res =
            querier.query_subaccount_deposit(&subaccount_id, &market.base_denom)?;
        let vault_base_total_balance = deposit_base_res.deposits.total_balance;

        if vault_base_total_balance < FPDecimal::zero() {
            return Err(ContractError::Std(StdError::generic_err(
                "Vault base deposits are negative",
            )));
        }

        let base_withdrawal_amount = vault_base_total_balance * lp_shares_to_burn / lp_total_supply;
        funds_to_return.append(&mut vec![Coin {
            denom: market.base_denom.to_owned(),
            amount: base_withdrawal_amount.into(),
        }]);

        let oracle_type = config
            .spot_oracle_types
            .get(index)
            .expect("oracle type should exist");
        let base_decimals = denom_decimals.get(&market.base_denom).unwrap();
        let oracle_price = get_oracle_price(
            &querier,
            oracle_type,
            &market.base_denom,
            &market.quote_denom,
            *base_decimals,
            *quote_decimals,
        )?;
        total_redemption_notional += base_withdrawal_amount * oracle_price;
    }

    let mut response = Response::new();
    let mut position_transfers = vec![];

    for (_, market_id) in config.derivative_market_ids.iter().enumerate() {
        let vault_position = querier
            .query_vanilla_subaccount_position(&market_id.to_owned(), &subaccount_id.to_owned())?
            .state;

        if let Some(mut p) = vault_position {
            let position_transfer = PositionTransferAction {
                market_id: market_id.to_owned(),
                source_subaccount_id: subaccount_id.to_owned(),
                destination_subaccount_id: redeemer_subaccount_id.to_owned(),
                quantity: p.quantity * lp_shares_to_burn / lp_total_supply,
            };
            position_transfers.push(position_transfer);

            let derivative_market_res = querier.query_derivative_market(market_id)?;
            apply_funding_to_position(Some(&mut p), &derivative_market_res);

            let position_notional = get_vault_estimated_position_notional(
                Some(&mut Position {
                    isLong: p.isLong,
                    quantity: p.quantity * lp_shares_to_burn / lp_total_supply,
                    entry_price: p.entry_price,
                    margin: p.margin * lp_shares_to_burn / lp_total_supply,
                    cumulative_funding_entry: p.cumulative_funding_entry,
                }),
                &derivative_market_res,
            );
            total_redemption_notional += position_notional;
        };
    }

    let total_profits = total_redemption_notional - lp_position.subscription_amount;
    let should_charge_performance_fees =
        total_profits > lp_position.subscription_amount * FPDecimal::from_str("1.1").unwrap();

    let mut admin_fee_positions = ADMIN_FEE_POSITIONS
        .may_load(deps.storage)?
        .unwrap_or_default();

    if should_charge_performance_fees {
        let performance_fee =
            total_profits * config.performance_fee_rate / total_redemption_notional;

        for (_, coin) in funds_to_return.iter().enumerate() {
            let admin_fee: Uint128 = (performance_fee * coin.amount.into()).into();

            let admin_send_message = BankMsg::Send {
                to_address: config.admin.to_string(),
                amount: vec![Coin {
                    denom: coin.denom.to_owned(),
                    amount: admin_fee,
                }],
            };
            response = response.add_message(admin_send_message);

            let redeemer_send_message = BankMsg::Send {
                to_address: sender.to_string(),
                amount: vec![Coin {
                    denom: coin.denom.to_owned(),
                    amount: coin.amount - admin_fee,
                }],
            };
            response = response.add_message(redeemer_send_message);
        }

        for (_, position_transfer) in position_transfers.iter().enumerate() {
            let admin_fee_position_quantity = performance_fee * position_transfer.quantity;

            let existing_admin_fee_position =
                admin_fee_positions.get(&position_transfer.market_id.to_owned());
            let new_admin_fee_position_quantity = match existing_admin_fee_position {
                Some(q) => *q + admin_fee_position_quantity,
                None => admin_fee_position_quantity,
            };
            admin_fee_positions.insert(
                position_transfer.market_id.to_owned(),
                new_admin_fee_position_quantity,
            );

            let redeemer_privileged_action = PrivilegedAction {
                synthetic_trade: None,
                position_transfer: Some(PositionTransferAction {
                    market_id: position_transfer.market_id.to_owned(),
                    source_subaccount_id: position_transfer.source_subaccount_id.to_owned(),
                    destination_subaccount_id: position_transfer
                        .destination_subaccount_id
                        .to_owned(),
                    quantity: position_transfer.quantity - admin_fee_position_quantity,
                }),
            };

            response = response.set_data(to_binary(&Some(redeemer_privileged_action))?);
        }
    }

    let new_lp_total_supply = lp_total_supply - lp_shares_to_burn;
    LP_TOTAL_SUPPLY.save(deps.storage, &new_lp_total_supply)?;

    lp_positions.remove(sender);
    LP_POSITIONS.save(deps.storage, &lp_positions)?;

    ADMIN_FEE_POSITIONS.save(deps.storage, &admin_fee_positions)?;

    let mut admin_owned_shares = ADMIN_OWNED_SHARES.load(deps.storage)?;
    let is_fund_closed = IS_FUND_CLOSED.may_load(deps.storage)?.unwrap_or_default();

    if sender == &config.admin && !is_fund_closed {
        admin_owned_shares -= lp_shares_to_burn;

        if admin_owned_shares * FPDecimal::from(10u128) < new_lp_total_supply {
            return Err(ContractError::Std(StdError::generic_err(
                "Admin must own at least 10% of fund",
            )));
        }

        ADMIN_OWNED_SHARES.save(deps.storage, &admin_owned_shares)?;
        return Ok(Response::new());
    }

    Ok(response)
}
