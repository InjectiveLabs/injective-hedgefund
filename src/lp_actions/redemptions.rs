use cosmwasm_std::{
    to_binary, Addr, BankMsg, Coin, DepsMut, Env, Response, StdError, Timestamp, Uint128,
};
use injective_cosmwasm::{
    privileged_action::{PositionTransferAction, PrivilegedAction},
    InjectiveMsgWrapper, InjectiveQuerier, InjectiveQueryWrapper, MarketId, Position, SubaccountId,
};
use injective_math::FPDecimal;

use crate::{
    state::{
        Config, ADMIN_FEE_POSITIONS, ADMIN_OWNED_SHARES, CONFIG, DENOM_DECIMALS, IS_FUND_CLOSED,
        LP_POSITIONS, LP_TOTAL_SUPPLY,
    },
    ContractError,
};

use super::{
    derivative_position_helpers::{
        apply_funding_to_position, get_vault_estimated_position_notional,
    },
    utils::get_spot_base_in_quote,
};

const ONE_YEAR_IN_SECONDS: u64 = 365 * 24 * 60 * 60;

pub fn ensure_valid_redemption(
    env: &Env,
    subscription_time: Timestamp,
    vault_quote_total_balance: FPDecimal,
) -> Result<(), ContractError> {
    if vault_quote_total_balance < FPDecimal::zero() {
        return Err(ContractError::Std(StdError::generic_err(
            "Vault quote deposits are negative",
        )));
    }

    if env.block.time <= subscription_time.plus_seconds(ONE_YEAR_IN_SECONDS) {
        return Err(ContractError::Std(StdError::generic_err(
            "Redeemer LP position has not been updated",
        )));
    }

    Ok(())
}

pub fn get_updated_redemption_notional_and_update_derivative_position_transfers(
    mut total_redemption_notional: FPDecimal,
    position_transfers: &mut Vec<PositionTransferAction>,
    querier: &InjectiveQuerier,
    market_id: &MarketId,
    fund_subaccount_id: SubaccountId,
    redeemer_subaccount_id: SubaccountId,
    lp_shares_to_burn: FPDecimal,
    lp_total_supply: FPDecimal,
) -> Result<FPDecimal, ContractError> {
    let vault_position = querier
        .query_vanilla_subaccount_position(market_id, &fund_subaccount_id)?
        .state;

    if let Some(mut p) = vault_position {
        let position_transfer = PositionTransferAction {
            market_id: market_id.to_owned(),
            source_subaccount_id: fund_subaccount_id,
            destination_subaccount_id: redeemer_subaccount_id,
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

    Ok(total_redemption_notional)
}

pub fn get_redemption_response(
    deps: DepsMut<InjectiveQueryWrapper>,
    sender: &Addr,
    config: &Config,
    funds_to_return: &[Coin],
    total_profits: FPDecimal,
    total_redemption_notional: FPDecimal,
    position_transfers: Vec<PositionTransferAction>,
    should_charge_performance_fees: bool,
) -> Result<Response<InjectiveMsgWrapper>, ContractError> {
    let mut response = Response::new();

    let mut admin_fee_positions = ADMIN_FEE_POSITIONS
        .may_load(deps.storage)?
        .unwrap_or_default();

    let performance_fee = total_profits * config.performance_fee_rate / total_redemption_notional;

    for (_, coin) in funds_to_return.iter().enumerate() {
        let admin_fee: Uint128 = if should_charge_performance_fees {
            (performance_fee * coin.amount.into()).into()
        } else {
            Uint128::zero()
        };

        if admin_fee > Uint128::zero() {
            let admin_send_message = BankMsg::Send {
                to_address: config.admin.to_string(),
                amount: vec![Coin {
                    denom: coin.denom.to_owned(),
                    amount: admin_fee,
                }],
            };
            response = response.add_message(admin_send_message);
        }

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
        let admin_fee_position_quantity: FPDecimal = if should_charge_performance_fees {
            performance_fee * position_transfer.quantity
        } else {
            FPDecimal::zero()
        };

        if admin_fee_position_quantity > FPDecimal::zero() {
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
        }

        let redeemer_privileged_action = PrivilegedAction {
            synthetic_trade: None,
            position_transfer: Some(PositionTransferAction {
                market_id: position_transfer.market_id.to_owned(),
                source_subaccount_id: position_transfer.source_subaccount_id.to_owned(),
                destination_subaccount_id: position_transfer.destination_subaccount_id.to_owned(),
                quantity: position_transfer.quantity - admin_fee_position_quantity,
            }),
        };

        response = response.set_data(to_binary(&Some(redeemer_privileged_action))?);
    }

    if should_charge_performance_fees {
        ADMIN_FEE_POSITIONS.save(deps.storage, &admin_fee_positions)?;
    }

    Ok(response)
}

pub fn get_fund_redemption_response(
    deps: DepsMut<InjectiveQueryWrapper>,
    env: &Env,
    sender: &Addr,
    redeemer_subaccount_id: SubaccountId,
) -> Result<Response<InjectiveMsgWrapper>, ContractError> {
    let querier = InjectiveQuerier::new(&deps.querier);
    let config = CONFIG.load(deps.storage)?;

    let denom_decimals = DENOM_DECIMALS.load(deps.storage)?;
    let lp_total_supply = LP_TOTAL_SUPPLY.load(deps.storage)?;
    let mut lp_positions = LP_POSITIONS.load(deps.storage)?;

    let mut admin_owned_shares = ADMIN_OWNED_SHARES.load(deps.storage)?;
    let is_fund_closed = IS_FUND_CLOSED.may_load(deps.storage)?.unwrap_or_default();

    let quote_decimals = denom_decimals.get(&config.quote_denom).unwrap();

    let deposit_quote_res =
        querier.query_subaccount_deposit(&config.fund_subaccount_id, &config.quote_denom)?;
    let vault_quote_total_balance = deposit_quote_res.deposits.total_balance;

    let lp_position = lp_positions
        .get(sender)
        .ok_or(ContractError::Std(StdError::generic_err(
            "Redeemer LP position does not exist",
        )))?;
    let lp_shares_to_burn = lp_position.shares;

    ensure_valid_redemption(
        env,
        lp_position.subscription_time,
        vault_quote_total_balance,
    )?;

    let quote_withdrawal_amount = vault_quote_total_balance * lp_shares_to_burn / lp_total_supply;
    let mut funds_to_return = vec![Coin {
        denom: config.quote_denom.to_owned(),
        amount: quote_withdrawal_amount.into(),
    }];

    let mut total_redemption_notional = quote_withdrawal_amount;

    for (index, market_id) in config.spot_market_ids.iter().enumerate() {
        let oracle_type = config
            .spot_oracle_types
            .get(index)
            .expect("oracle type should exist");
        total_redemption_notional += get_spot_base_in_quote(
            &querier,
            &config.fund_subaccount_id.to_owned(),
            &market_id.to_owned(),
            &denom_decimals,
            quote_decimals,
            oracle_type,
            Some((lp_shares_to_burn, lp_total_supply, &mut funds_to_return)),
        )?;
    }

    let mut position_transfers = vec![];

    for (_, market_id) in config.derivative_market_ids.iter().enumerate() {
        total_redemption_notional +=
            get_updated_redemption_notional_and_update_derivative_position_transfers(
                total_redemption_notional,
                &mut position_transfers,
                &querier,
                market_id,
                config.fund_subaccount_id.to_owned(),
                redeemer_subaccount_id.to_owned(),
                lp_shares_to_burn,
                lp_total_supply,
            )?;
    }

    let total_profits = total_redemption_notional - lp_position.subscription_amount;
    let time_since_redemption = env.block.time.seconds() - lp_position.subscription_time.seconds();
    let profits_per_year = total_profits * FPDecimal::from(ONE_YEAR_IN_SECONDS as u128)
        / FPDecimal::from(time_since_redemption as u128);

    let should_charge_performance_fees =
        profits_per_year > lp_position.subscription_amount * config.min_yearly_roi_for_fees;

    let new_lp_total_supply = lp_total_supply - lp_shares_to_burn;
    LP_TOTAL_SUPPLY.save(deps.storage, &new_lp_total_supply)?;
    lp_positions.remove(sender);
    LP_POSITIONS.save(deps.storage, &lp_positions)?;

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

    get_redemption_response(
        deps,
        sender,
        &config,
        &funds_to_return,
        total_profits,
        total_redemption_notional,
        position_transfers,
        should_charge_performance_fees,
    )
}
