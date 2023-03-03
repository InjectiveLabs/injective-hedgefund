use std::collections::HashMap;

use cw_storage_plus::Item;
use injective_cosmwasm::{MarketId, OracleType, SubaccountId};
use injective_math::FPDecimal;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use cosmwasm_std::{Addr, Timestamp};

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq, JsonSchema)]
pub struct Config {
    pub admin: Addr,
    pub spot_oracle_types: Vec<OracleType>,
    pub spot_market_ids: Vec<MarketId>,
    pub derivative_market_ids: Vec<MarketId>,
    pub quote_denom: String,
    pub fund_subaccount_id: SubaccountId,
    pub performance_fee_rate: FPDecimal,
    pub min_yearly_roi_for_fees: FPDecimal, // e.g. 1.1 means min 10% yearly ROI before paying admin fees
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq, JsonSchema)]
pub struct LPPosition {
    pub shares: FPDecimal,
    pub subscription_time: Timestamp,
    pub subscription_amount: FPDecimal,
}

pub const CONFIG: Item<Config> = Item::new("config");

pub const LP_POSITIONS: Item<HashMap<Addr, LPPosition>> = Item::new("lp_positions");

pub const LP_TOTAL_SUPPLY: Item<FPDecimal> = Item::new("lp_total_supply");

pub const ADMIN_FEE_POSITIONS: Item<HashMap<MarketId, FPDecimal>> =
    Item::new("admin_fee_positions");

pub const ADMIN_OWNED_SHARES: Item<FPDecimal> = Item::new("admin_owned_shares");

pub const DENOM_DECIMALS: Item<HashMap<String, u64>> = Item::new("denom_decimals");

pub const IS_FUND_CLOSED: Item<bool> = Item::new("is_fund_closed");
