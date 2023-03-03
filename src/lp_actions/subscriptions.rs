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

use super::utils::get_fund_total_notional;

pub fn get_fund_subscription_response(
    deps: DepsMut<InjectiveQueryWrapper>,
    env: &Env,
    sender: &Addr,
    total_funds_supplied: Vec<Coin>,
) -> Result<Response<InjectiveMsgWrapper>, ContractError> {
    let querier = InjectiveQuerier::new(&deps.querier);
    let config = CONFIG.load(deps.storage)?;

    let denom_decimals = DENOM_DECIMALS.load(deps.storage)?;
    let lp_total_supply = LP_TOTAL_SUPPLY.load(deps.storage)?;

    let is_fund_closed = IS_FUND_CLOSED.may_load(deps.storage)?.unwrap_or_default();
    if is_fund_closed {
        return Err(ContractError::Std(StdError::generic_err("Fund is closed")));
    }

    let mut has_invalid_coins = false;
    let mut total_quote_funds_supplied: u128 = 0;

    total_funds_supplied.iter().for_each(|f| {
        if f.denom == config.quote_denom {
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

    let fund_total_notional = get_fund_total_notional(&querier, &config, &denom_decimals)?;
    let lp_shares_to_mint = get_token_mint_data(
        total_quote_funds_supplied.into(),
        fund_total_notional,
        lp_total_supply,
    )?;

    store_subscription(
        deps,
        env,
        sender,
        lp_shares_to_mint,
        lp_total_supply,
        total_quote_funds_supplied.into(),
        &config.admin,
    )?;

    Ok(Response::new())
}

pub fn store_subscription(
    deps: DepsMut<InjectiveQueryWrapper>,
    env: &Env,
    sender: &Addr,
    lp_shares_to_mint: FPDecimal,
    lp_total_supply: FPDecimal,
    total_quote_funds_supplied: FPDecimal,
    admin: &Addr,
) -> StdResult<()> {
    let mut lp_positions = LP_POSITIONS.may_load(deps.storage)?.unwrap_or_default();
    let mut admin_owned_shares = ADMIN_OWNED_SHARES.load(deps.storage)?;

    let new_lp_total_supply = lp_total_supply + lp_shares_to_mint;

    let old_lp_position = lp_positions.insert(
        sender.to_owned(),
        LPPosition {
            shares: lp_shares_to_mint,
            subscription_time: env.block.time,
            subscription_amount: total_quote_funds_supplied,
        },
    );
    if old_lp_position.is_some() {
        // consider allowing multiple subscriptions from same address through calculating average profits
        return Err(StdError::generic_err("Already subscribed"));
    }

    LP_POSITIONS.save(deps.storage, &lp_positions)?;
    LP_TOTAL_SUPPLY.save(deps.storage, &new_lp_total_supply)?;

    let is_subscriber_the_admin = sender == admin;
    if is_subscriber_the_admin {
        admin_owned_shares += lp_shares_to_mint;
        ADMIN_OWNED_SHARES.save(deps.storage, &admin_owned_shares)?;
    }

    let is_admin_owned_shares_below_10_percent =
        admin_owned_shares * FPDecimal::from(10u128) < new_lp_total_supply;
    if is_admin_owned_shares_below_10_percent {
        return Err(StdError::generic_err("Admin must own at least 10% of fund"));
    }

    Ok(())
}

pub fn get_token_mint_data(
    total_quote_funds_supplied: FPDecimal,
    fund_total_notional: FPDecimal,
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

    let lp_shares_to_mint = lp_total_supply * total_quote_funds_supplied / fund_total_notional;
    if lp_shares_to_mint.is_zero() {
        return Err(StdError::generic_err(
            "Insufficient funds to mint LP tokens",
        ));
    }

    Ok(lp_shares_to_mint)
}
