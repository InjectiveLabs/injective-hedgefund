use std::collections::{HashMap, HashSet};

#[cfg(not(feature = "library"))]
use cosmwasm_std::entry_point;
use cosmwasm_std::{
    ensure_eq, to_binary, Addr, Binary, CosmosMsg, Deps, DepsMut, Env, MessageInfo, Response,
    StdError, StdResult,
};
use injective_cosmwasm::privileged_action::{PositionTransferAction, PrivilegedAction};
use injective_cosmwasm::{
    InjectiveMsg, InjectiveMsgWrapper, InjectiveQuerier, InjectiveQueryWrapper, SubaccountId,
};
use injective_math::FPDecimal;

use crate::closing_fund::close_fund;
use crate::error::ContractError;
use crate::lp_actions::redemptions::get_fund_redemption_response;
use crate::lp_actions::subscriptions::get_fund_subscription_response;
use crate::msg::{ExecuteMsg, InstantiateMsg, QueryMsg};
use crate::state::{
    Config, ADMIN_FEE_POSITIONS, CONFIG, DENOM_DECIMALS, IS_FUND_CLOSED, LP_TOTAL_SUPPLY,
};
use cw2::set_contract_version;

// version info for migration info
const CONTRACT_NAME: &str = "crates.io:injective:dummy";
const CONTRACT_VERSION: &str = env!("CARGO_PKG_VERSION");

#[cfg_attr(not(feature = "library"), entry_point)]
pub fn instantiate(
    deps: DepsMut<InjectiveQueryWrapper>,
    _env: Env,
    info: MessageInfo,
    msg: InstantiateMsg,
) -> Result<Response<InjectiveMsgWrapper>, ContractError> {
    set_contract_version(deps.storage, CONTRACT_NAME, CONTRACT_VERSION)?;
    CONFIG.save(
        deps.storage,
        &Config {
            admin: info.sender.to_owned(),
            spot_oracle_types: msg.spot_oracle_types.to_owned(),
            spot_market_ids: msg.spot_market_ids.to_owned(),
            derivative_market_ids: msg.derivative_market_ids.to_owned(),
            quote_denom: msg.quote_denom.to_owned(),
            fund_subaccount_id: msg.fund_subaccount_id,
            performance_fee_rate: msg.performance_fee_rate,
            min_yearly_roi_for_fees: msg.min_yearly_roi_for_fees,
        },
    )?;

    LP_TOTAL_SUPPLY.save(deps.storage, &FPDecimal::zero())?;

    let querier = InjectiveQuerier::new(&deps.querier);

    if msg.spot_market_ids.is_empty() && msg.derivative_market_ids.is_empty() {
        return Err(ContractError::NoMarketsProvided {});
    }

    if msg.spot_market_ids.len() != msg.spot_oracle_types.len() {
        return Err(ContractError::InvalidSpotOracleTypes {});
    }

    let mut denoms = HashSet::new();
    denoms.insert(msg.quote_denom.to_owned());

    for (_, market_id) in msg.spot_market_ids.iter().enumerate() {
        let market_res = querier
            .query_spot_market(market_id)
            .expect("spot market {market_id} not found in query");
        let spot_market = market_res
            .market
            .expect("spot market {market_id} not found in result");

        denoms.insert(spot_market.base_denom);

        if spot_market.quote_denom != msg.quote_denom {
            return Err(ContractError::IncorrectMarketQuoteDenom {});
        }
    }

    for (_, market_id) in msg.derivative_market_ids.iter().enumerate() {
        let market_res = querier
            .query_derivative_market(market_id)
            .expect("derivative market {market_id} not found in query");
        let derivative_market = market_res
            .market
            .market
            .expect("derivative market {market_id} not found in result");

        if derivative_market.quote_denom != msg.quote_denom {
            return Err(ContractError::IncorrectMarketQuoteDenom {});
        }
    }

    let deposit_quote_res =
        querier.query_denom_decimals(&denoms.into_iter().collect::<Vec<String>>())?;

    let denom_hash_map: HashMap<String, u64> = deposit_quote_res
        .denom_decimals
        .iter()
        .map(|d| (d.denom.to_owned(), d.decimals))
        .collect();

    DENOM_DECIMALS.save(deps.storage, &denom_hash_map)?;

    Ok(Response::new()
        .add_attribute("method", "instantiate")
        .add_attribute("owner", info.sender))
}

#[cfg_attr(not(feature = "library"), entry_point)]
pub fn execute(
    deps: DepsMut<InjectiveQueryWrapper>,
    env: Env,
    info: MessageInfo,
    msg: ExecuteMsg,
) -> Result<Response<InjectiveMsgWrapper>, ContractError> {
    match msg {
        ExecuteMsg::AdminExecuteMessages { injective_messages } => {
            execute_messages(deps, info.sender, injective_messages)
        }
        ExecuteMsg::Subscribe {} => {
            get_fund_subscription_response(deps, &env, &info.sender, info.funds)
        }
        ExecuteMsg::Redeem {
            redeemer_subaccount_id,
        } => get_fund_redemption_response(deps, &env, &info.sender, redeemer_subaccount_id),
        ExecuteMsg::AdminReceiveFeePositions {
            receiving_subaccount_id,
        } => admin_receive_fee_positions(deps, info.sender, receiving_subaccount_id),
        ExecuteMsg::CloseFund {} => close_fund(deps, info.sender),
    }
}

pub fn admin_receive_fee_positions(
    deps: DepsMut<InjectiveQueryWrapper>,
    sender: Addr,
    receiving_subaccount_id: SubaccountId,
) -> Result<Response<InjectiveMsgWrapper>, ContractError> {
    let config = CONFIG.load(deps.storage)?;
    ensure_eq!(sender, config.admin, ContractError::Unauthorized {});

    let admin_fee_positions = ADMIN_FEE_POSITIONS.load(deps.storage)?;

    let mut response = Response::new();

    for (_, (market_id, quantity)) in admin_fee_positions.iter().enumerate() {
        let privileged_action = PrivilegedAction {
            synthetic_trade: None,
            position_transfer: Some(PositionTransferAction {
                market_id: market_id.to_owned(),
                source_subaccount_id: config.fund_subaccount_id.to_owned(),
                destination_subaccount_id: receiving_subaccount_id.to_owned(),
                quantity: quantity.to_owned(),
            }),
        };
        response = response.set_data(to_binary(&Some(privileged_action))?);
    }

    Ok(response)
}

pub fn execute_messages(
    deps: DepsMut<InjectiveQueryWrapper>,
    sender: Addr,
    msgs: Vec<CosmosMsg<InjectiveMsgWrapper>>,
) -> Result<Response<InjectiveMsgWrapper>, ContractError> {
    let config = CONFIG.load(deps.storage)?;
    ensure_eq!(sender, config.admin, ContractError::Unauthorized {});

    let is_fund_closed = IS_FUND_CLOSED.may_load(deps.storage)?.unwrap_or_default();
    if is_fund_closed {
        return Err(ContractError::Std(StdError::generic_err("Fund is closed")));
    }

    if !are_messages_authorized(msgs.iter().collect()) {
        return Err(ContractError::Unauthorized {});
    }

    Ok(Response::new().add_messages(msgs))
}

fn are_messages_authorized(msgs: Vec<&CosmosMsg<InjectiveMsgWrapper>>) -> bool {
    msgs.into_iter().all(is_message_authorized)
}

fn is_message_authorized(msg: &CosmosMsg<InjectiveMsgWrapper>) -> bool {
    // TODO add more message validations, e.g. prevent out-of-range market orders

    matches!(
        msg,
        CosmosMsg::Custom(InjectiveMsgWrapper {
            msg_data: InjectiveMsg::BatchUpdateOrders { .. },
            ..
        })
    )
}

#[cfg_attr(not(feature = "library"), entry_point)]
pub fn query(_deps: Deps, _env: Env, msg: QueryMsg) -> StdResult<Binary> {
    match msg {
        QueryMsg::Ping { .. } => to_binary("pong"),
    }
}
