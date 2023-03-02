use injective_cosmwasm::{DerivativeMarketResponse, Position};
use injective_math::FPDecimal;

pub fn get_vault_estimated_position_notional(
    vault_position: Option<&mut Position>,
    market_res: &DerivativeMarketResponse,
) -> FPDecimal {
    if vault_position.is_none() {
        return FPDecimal::zero();
    }

    let cumulative_funding = market_res
        .market
        .info
        .as_ref()
        .expect("market info should be set")
        .perpetual_info
        .funding_info
        .cumulative_funding;

    let valuation_price = market_res.market.mark_price;

    vault_position
        .unwrap()
        .get_position_value(valuation_price, cumulative_funding)
}

pub fn apply_funding_to_position(
    vault_position: Option<&mut Position>,
    market_res: &DerivativeMarketResponse,
) {
    if vault_position.is_none() {
        return;
    }

    let cumulative_funding = market_res
        .market
        .info
        .as_ref()
        .expect("market info should be set")
        .perpetual_info
        .funding_info
        .cumulative_funding;

    vault_position.unwrap().apply_funding(cumulative_funding)
}
