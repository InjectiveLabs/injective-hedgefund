use cosmwasm_std::{Addr, Coin, DepsMut, Env, Response, StdError, StdResult};
use injective_cosmwasm::{InjectiveMsgWrapper, InjectiveQuerier, InjectiveQueryWrapper};
use injective_math::FPDecimal;

use crate::{
    state::{
        LPPosition, ADMIN_OWNED_SHARES, CONFIG, DENOM_DECIMALS, IS_FUND_CLOSED, LP_POSITIONS,
        LP_TOTAL_SUPPLY,
    },
    ContractError,
};

use super::{
    derivative_position_helpers::{
        apply_funding_to_position, get_vault_estimated_position_notional,
    },
    oracle_price::get_oracle_price,
};

pub fn get_vault_subscription_response(
    deps: DepsMut<InjectiveQueryWrapper>,
    env: &Env,
    sender: &Addr,
    total_funds_supplied: Vec<Coin>,
) -> Result<Response<InjectiveMsgWrapper>, ContractError> {
    let querier = InjectiveQuerier::new(&deps.querier);

    let is_fund_closed = IS_FUND_CLOSED.may_load(deps.storage)?.unwrap_or_default();
    if is_fund_closed {
        return Err(ContractError::Std(StdError::generic_err("Fund is closed")));
    }

    let config = CONFIG.load(deps.storage)?;
    let denom_decimals = DENOM_DECIMALS.load(deps.storage)?;

    let subaccount_id = config.subaccount_id;
    let quote_denom = config.quote_denom;

    let quote_decimals = denom_decimals.get(&quote_denom).unwrap();
    let deposit_quote_res = querier.query_subaccount_deposit(&subaccount_id, &quote_denom)?;
    let vault_quote_total_balance = deposit_quote_res.deposits.total_balance;

    if vault_quote_total_balance < FPDecimal::zero() {
        return Err(ContractError::Std(StdError::generic_err(
            "Vault quote deposits are negative",
        )));
    }

    let mut vault_total_notional = vault_quote_total_balance;

    for (index, market_id) in config.spot_market_ids.iter().enumerate() {
        let spot_market_res = querier.query_spot_market(market_id)?;
        let market = spot_market_res.market.expect("market should be available");

        let base_decimals = denom_decimals.get(&market.base_denom).unwrap();

        let deposit_base_res =
            querier.query_subaccount_deposit(&subaccount_id, &market.base_denom)?;
        let vault_base_total_balance = deposit_base_res.deposits.total_balance;

        if vault_base_total_balance < FPDecimal::zero() {
            return Err(ContractError::Std(StdError::generic_err(
                "Vault base deposits are negative",
            )));
        }

        let oracle_type = config
            .spot_oracle_types
            .get(index)
            .expect("oracle type should exist");
        let oracle_price = get_oracle_price(
            &querier,
            oracle_type,
            &market.base_denom,
            &market.quote_denom,
            *base_decimals,
            *quote_decimals,
        )?;

        vault_total_notional += vault_base_total_balance * oracle_price;
    }

    for (_, market_id) in config.derivative_market_ids.iter().enumerate() {
        let derivative_market_res = querier.query_derivative_market(market_id)?;

        let mut vault_position = querier
            .query_vanilla_subaccount_position(market_id, &subaccount_id)?
            .state;
        apply_funding_to_position(vault_position.as_mut(), &derivative_market_res);

        let position_notional =
            get_vault_estimated_position_notional(vault_position.as_mut(), &derivative_market_res);
        vault_total_notional += position_notional;
    }

    let mut has_invalid_coins = false;
    let mut total_quote_funds_supplied: u128 = 0;

    total_funds_supplied.iter().for_each(|f| {
        if f.denom == quote_denom {
            total_quote_funds_supplied = f.amount.into()
        } else {
            has_invalid_coins = true;
        }
    });
    if has_invalid_coins {
        return Err(ContractError::Std(StdError::generic_err(
            "Invalid coin denomination",
        )));
    }

    let lp_total_supply = LP_TOTAL_SUPPLY.load(deps.storage)?;

    let lp_shares_to_mint = get_token_mint_data(
        total_quote_funds_supplied.into(),
        vault_total_notional,
        lp_total_supply,
    )?;

    if lp_shares_to_mint.is_zero() {
        return Err(ContractError::Std(StdError::generic_err(
            "Insufficient funds to mint LP tokens",
        )));
    }

    let new_lp_total_supply = lp_total_supply + lp_shares_to_mint;
    LP_TOTAL_SUPPLY.save(deps.storage, &new_lp_total_supply)?;

    let mut lp_positions = LP_POSITIONS.may_load(deps.storage)?.unwrap_or_default();

    let old_lp_position = lp_positions.insert(
        sender.to_owned(),
        LPPosition {
            shares: lp_shares_to_mint,
            subscription_time: env.block.time,
            subscription_amount: total_quote_funds_supplied.into(),
        },
    );
    if old_lp_position.is_some() {
        // consider allowing multiple subscriptions from same address through calculating average profits
        return Err(ContractError::Std(StdError::generic_err(
            "Already subscribed",
        )));
    }

    LP_POSITIONS.save(deps.storage, &lp_positions)?;

    let mut admin_owned_shares = ADMIN_OWNED_SHARES.load(deps.storage)?;

    if sender == &config.admin {
        admin_owned_shares += lp_shares_to_mint;
        ADMIN_OWNED_SHARES.save(deps.storage, &admin_owned_shares)?;
        return Ok(Response::new());
    }

    if admin_owned_shares * FPDecimal::from(10u128) < new_lp_total_supply {
        return Err(ContractError::Std(StdError::generic_err(
            "Admin must own at least 10% of fund",
        )));
    }

    Ok(Response::new())
}

pub fn get_token_mint_data(
    total_quote_funds_supplied: FPDecimal,
    vault_total_notional: FPDecimal,
    lp_total_supply: FPDecimal,
) -> StdResult<FPDecimal> {
    let is_first_subscription = lp_total_supply.is_zero();

    if is_first_subscription {
        let lp_shares_to_mint = 1_000_000_000_000_000_000u128;
        return Ok(lp_shares_to_mint.into());
    }

    if total_quote_funds_supplied <= FPDecimal::zero() {
        return Err(StdError::generic_err(
            "Supplied quote funds must be greater than 0",
        ));
    }

    let lp_shares_to_mint = lp_total_supply * total_quote_funds_supplied / vault_total_notional;
    Ok(lp_shares_to_mint)
}
