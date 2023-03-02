use cosmwasm_std::StdResult;
use injective_cosmwasm::{InjectiveQuerier, OracleType};
use injective_math::FPDecimal;

pub fn get_oracle_price(
    querier: &InjectiveQuerier,
    oracle_type: &OracleType,
    base_denom: &str,
    quote_denom: &str,
    base_decimals: u64,
    quote_decimals: u64,
) -> StdResult<FPDecimal> {
    // TODO consider spot oracle failure similar to derivatives @markus

    let raw_oracle_price = querier
        .query_oracle_price(oracle_type, base_denom, quote_denom)?
        .price;

    let base_decimals_fp: FPDecimal = (base_decimals as u128).into();
    let quote_decimals_fp: FPDecimal = (quote_decimals as u128).into();

    // oracle_price = raw_oracle_price * 10u128 ^ (quote_decimals - base_decimals)
    let decimals_adjusted_price =
        raw_oracle_price * FPDecimal::_pow(10u128.into(), quote_decimals_fp - base_decimals_fp);
    Ok(decimals_adjusted_price)
}
