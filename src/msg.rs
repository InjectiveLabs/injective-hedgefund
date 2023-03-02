use cosmwasm_std::CosmosMsg;
use injective_cosmwasm::{InjectiveMsgWrapper, MarketId, OracleType, SubaccountId};
use injective_math::FPDecimal;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq, JsonSchema)]
pub struct InstantiateMsg {
    pub spot_oracle_types: Vec<OracleType>,
    pub spot_market_ids: Vec<MarketId>,
    pub derivative_market_ids: Vec<MarketId>,
    pub quote_denom: String, // all markets must have this as the quote denom
    pub subaccount_id: SubaccountId,
    pub performance_fee_rate: FPDecimal,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ExecuteMsg {
    AdminExecuteMessages {
        injective_messages: Vec<CosmosMsg<InjectiveMsgWrapper>>,
    },
    Subscribe {},
    Redeem {
        redeemer_subaccount_id: SubaccountId,
    },
    AdminReceiveFeePositions {
        receiving_subaccount_id: SubaccountId,
    },
    CloseFund {},
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SudoMsg {
    BeginBlocker {},
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum QueryMsg {
    Ping {},
}
