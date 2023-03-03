use std::collections::HashMap;

use cosmwasm_std::{Coin, StdError};
use injective_cosmwasm::{InjectiveQuerier, MarketId, OracleType, SubaccountId};
use injective_math::FPDecimal;

use crate::{state::Config, ContractError};

use super::{
    derivative_position_helpers::{
        apply_funding_to_position, get_vault_estimated_position_notional,
    },
    oracle_price::get_oracle_price,
};

pub fn get_spot_base_in_quote(
    querier: &InjectiveQuerier,
    subaccount_id: &SubaccountId,
    market_id: &MarketId,
    denom_decimals: &HashMap<String, u64>,
    quote_decimals: &u64,
    oracle_type: &OracleType,
    redemption_data: Option<(FPDecimal, FPDecimal, &mut Vec<Coin>)>,
) -> Result<FPDecimal, ContractError> {
    let spot_market_res = querier.query_spot_market(market_id)?;
    let market = spot_market_res.market.expect("market should be available");

    let base_decimals = denom_decimals.get(&market.base_denom).unwrap();

    let deposit_base_res = querier.query_subaccount_deposit(subaccount_id, &market.base_denom)?;
    let vault_base_total_balance = deposit_base_res.deposits.total_balance;

    if vault_base_total_balance < FPDecimal::zero() {
        return Err(ContractError::Std(StdError::generic_err(
            "Vault base deposits are negative",
        )));
    }

    let oracle_price = get_oracle_price(
        querier,
        oracle_type,
        &market.base_denom,
        &market.quote_denom,
        *base_decimals,
        *quote_decimals,
    )?;

    match redemption_data {
        Some((lp_shares_to_burn, lp_total_supply, funds_to_return)) => {
            let base_withdrawal_amount =
                vault_base_total_balance * lp_shares_to_burn / lp_total_supply;

            funds_to_return.append(&mut vec![Coin {
                denom: market.base_denom.to_owned(),
                amount: base_withdrawal_amount.into(),
            }]);

            Ok(base_withdrawal_amount * oracle_price)
        }
        None => Ok(vault_base_total_balance * oracle_price),
    }
}

pub fn get_derivative_base_in_quote(
    querier: &InjectiveQuerier,
    subaccount_id: &SubaccountId,
    market_id: &MarketId,
) -> Result<FPDecimal, ContractError> {
    let derivative_market_res = querier.query_derivative_market(market_id)?;

    let mut vault_position = querier
        .query_vanilla_subaccount_position(market_id, subaccount_id)?
        .state;
    apply_funding_to_position(vault_position.as_mut(), &derivative_market_res);

    let position_notional =
        get_vault_estimated_position_notional(vault_position.as_mut(), &derivative_market_res);

    Ok(position_notional)
}

pub fn get_fund_total_notional(
    querier: &InjectiveQuerier,
    config: &Config,
    denom_decimals: &HashMap<String, u64>,
) -> Result<FPDecimal, ContractError> {
    let quote_decimals = denom_decimals.get(&config.quote_denom).unwrap();

    let deposit_quote_res =
        querier.query_subaccount_deposit(&config.fund_subaccount_id, &config.quote_denom)?;
    let vault_quote_total_balance = deposit_quote_res.deposits.total_balance;

    if vault_quote_total_balance < FPDecimal::zero() {
        return Err(ContractError::Std(StdError::generic_err(
            "Vault quote deposits are negative",
        )));
    }

    let mut vault_total_notional = vault_quote_total_balance;

    for (index, market_id) in config.spot_market_ids.iter().enumerate() {
        let oracle_type = config
            .spot_oracle_types
            .get(index)
            .expect("oracle type should exist");
        vault_total_notional += get_spot_base_in_quote(
            querier,
            &config.fund_subaccount_id,
            market_id,
            denom_decimals,
            quote_decimals,
            oracle_type,
            None,
        )?;
    }

    for (_, market_id) in config.derivative_market_ids.iter().enumerate() {
        vault_total_notional +=
            get_derivative_base_in_quote(querier, &config.fund_subaccount_id, market_id)?;
    }

    Ok(vault_total_notional)
}
