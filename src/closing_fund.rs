#[cfg(not(feature = "library"))]
use cosmwasm_std::{ensure_eq, Addr, DepsMut, Response};
use injective_cosmwasm::{InjectiveMsgWrapper, InjectiveQuerier, InjectiveQueryWrapper};

use crate::{
    error::ContractError,
    state::{CONFIG, IS_FUND_CLOSED},
};

pub fn close_fund(
    deps: DepsMut<InjectiveQueryWrapper>,
    sender: Addr,
) -> Result<Response<InjectiveMsgWrapper>, ContractError> {
    let config = CONFIG.load(deps.storage)?;
    ensure_eq!(sender, config.admin, ContractError::Unauthorized {});

    let querier = InjectiveQuerier::new(&deps.querier);

    for (_, market_id) in config.derivative_market_ids.iter().enumerate() {
        let vault_position = querier
            .query_vanilla_subaccount_position(market_id, &config.fund_subaccount_id)?
            .state;
        ensure_eq!(vault_position, None, ContractError::NonZeroVaultPosition {});
    }

    IS_FUND_CLOSED.save(deps.storage, &true)?;

    Ok(Response::new())
}
