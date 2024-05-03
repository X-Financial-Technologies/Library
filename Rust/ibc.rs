#[cfg(not(feature = "library"))]
use cosmwasm_std::entry_point;
use cosmwasm_std::{to_binary, Binary, Deps, DepsMut, Env, MessageInfo, Response, StdResult};
use cw2::set_contract_version;
use provwasm_std::{ProvenanceMsg, ProvenanceQuery};

use crate::error::ContractError;
use crate::marker::collateral_matches_native_total_supply;
use crate::msg::{ExecuteMsg, InstantiateMsg, QueryMsg};
use crate::state::{State, STATE};
use crate::{execute, query};

// version info for migration info
const CONTRACT_NAME: &str = "crates.io:exchange";
const CONTRACT_VERSION: &str = env!("CARGO_PKG_VERSION");

#[cfg_attr(not(feature = "library"), entry_point)]
pub fn instantiate(
    deps: DepsMut<ProvenanceQuery>,
    _env: Env,
    info: MessageInfo,
    msg: InstantiateMsg,
) -> Result<Response<ProvenanceMsg>, ContractError> {
    let supply_matches = collateral_matches_native_total_supply(
        &deps,
        &msg.collateral_denom,
        &msg.native_denom,
        &deps.api.addr_validate(msg.marker_address.as_str())?,
    )?;
    if !supply_matches {
        return Err(ContractError::CollateralAndNativeSupplyMistmatchError {
            collateral_denom: msg.collateral_denom.clone(),
            native_denom: msg.native_denom.clone(),
            marker_address: msg.marker_address,
        });
    }

    let state = State {
        collateral_denom: msg.collateral_denom.clone(),
        native_denom: msg.native_denom.clone(),
        marker_address: deps.api.addr_validate(msg.marker_address.as_str())?,
    };
    set_contract_version(deps.storage, CONTRACT_NAME, CONTRACT_VERSION)?;
    STATE.save(deps.storage, &state)?;

    Ok(Response::new()
        .add_attribute("action", "provwasm.contracts.exchange.init")
        .add_attribute("integration_test", "v1")
        .add_attribute("creator", info.sender)
        .add_attribute("collateral_denom", msg.collateral_denom)
        .add_attribute("native_denom", msg.native_denom)
        .add_attribute("marker_address", msg.marker_address))
}

#[cfg_attr(not(feature = "library"), entry_point)]
pub fn execute(
    deps: DepsMut<ProvenanceQuery>,
    env: Env,
    info: MessageInfo,
    msg: ExecuteMsg,
) -> Result<Response<ProvenanceMsg>, ContractError> {
    match msg {
        ExecuteMsg::Trade {} => execute::trade(deps, env, info),
    }
}

#[cfg_attr(not(feature = "library"), entry_point)]
pub fn query(deps: Deps<ProvenanceQuery>, _env: Env, msg: QueryMsg) -> StdResult<Binary> {
    match msg {
        QueryMsg::GetExchangeInfo {} => to_binary(&query::get_exchange_info(deps)?),
    }
}

#[cfg(test)]
mod tests {

    use super::*;
    use crate::msg::GetExchangeInfoResponse;
    use cosmwasm_std::testing::{mock_env, mock_info};
    use cosmwasm_std::CosmosMsg::Bank;
    use cosmwasm_std::{from_binary, Addr, Attribute, BankMsg, Coin, Decimal, Uint128};
    use provwasm_mocks::mock_dependencies_with_balances;
    use provwasm_std::{burn_marker_supply, mint_marker_supply, withdraw_coins, Marker};

    fn create_marker(address: &str, denom: &str, coins: Vec<Coin>, total_supply: u128) -> Marker {
        Marker {
            address: Addr::unchecked(address.to_string()),
            coins: coins,
            account_number: 100,
            sequence: 100,
            manager: "test".to_string(),
            permissions: Vec::new(),
            status: provwasm_std::MarkerStatus::Active,
            denom: denom.to_string(),
            total_supply: Decimal::from_atomics(Uint128::new(total_supply), 0).unwrap(),
            marker_type: provwasm_std::MarkerType::Coin,
            supply_fixed: true,
        }
    }

    #[test]
    fn proper_initialization() {
        // Create the marker and fund it
        let marker = create_marker(
            "tp1kn7phy33x5pqpax6t9n60tkjtuqf5jt37txe0h",
            "denom1",
            vec![Coin::new(1000, "denom1"), Coin::new(1000, "denom2")],
            1000,
        );
        let mut deps = mock_dependencies_with_balances(&[(marker.address.as_str(), &marker.coins)]);
        deps.querier.with_markers(vec![marker.clone()]);

        let msg = InstantiateMsg {
            native_denom: marker.denom.clone(),
            collateral_denom: "denom2".to_string(),
            marker_address: marker.address.to_string(),
        };
        let info = mock_info("tp1w9fnesmguvlal3mp62na3f58zww9jtmtwfnx9h", &[]);

        // Verify we have all the attributes
        let res = instantiate(deps.as_mut(), mock_env(), info, msg).unwrap();
        assert_eq!(0, res.messages.len());
        assert_eq!(6, res.attributes.len());
        assert_eq!(
            Attribute::new("action", "provwasm.contracts.exchange.init"),
            res.attributes[0]
        );
        assert_eq!(Attribute::new("integration_test", "v1"), res.attributes[1]);
        assert_eq!(
            Attribute::new("creator", "tp1w9fnesmguvlal3mp62na3f58zww9jtmtwfnx9h"),
            res.attributes[2]
        );
        assert_eq!(
            Attribute::new("collateral_denom", "denom2"),
            res.attributes[3]
        );
        assert_eq!(
            Attribute::new("native_denom", &marker.denom),
            res.attributes[4]
        );
        assert_eq!(
            Attribute::new("marker_address", marker.address.to_string()),
            res.attributes[5]
        );

        // Check the native_denom, private_denom, and exchange_rate
        let res = query(deps.as_ref(), mock_env(), QueryMsg::GetExchangeInfo {}).unwrap();
        let value: GetExchangeInfoResponse = from_binary(&res).unwrap();
        assert_eq!(marker.denom.clone(), value.native_denom);
        assert_eq!("denom2", value.collateral_denom);
        assert_eq!(marker.address.to_string(), value.marker_address);
    }

    #[test]
    fn invalid_initialization_fund_mismatch() {
        // Create the marker and fund it
        let marker = create_marker(
            "tp1kn7phy33x5pqpax6t9n60tkjtuqf5jt37txe0h",
            "denom1",
            vec![Coin::new(500, "denom1"), Coin::new(1000, "denom2")],
            500,
        );
        let mut deps = mock_dependencies_with_balances(&[(marker.address.as_str(), &marker.coins)]);
        deps.querier.with_markers(vec![marker.clone()]);

        let msg = InstantiateMsg {
            native_denom: marker.denom.clone(),
            collateral_denom: "denom2".to_string(),
            marker_address: marker.address.to_string(),
        };
        let info = mock_info("tp1w9fnesmguvlal3mp62na3f58zww9jtmtwfnx9h", &[]);

        // Verify we have all the attributes
        let res = instantiate(deps.as_mut(), mock_env(), info, msg);
        match res {
            Err(ContractError::CollateralAndNativeSupplyMistmatchError {
                collateral_denom: _,
                native_denom: _,
                marker_address: _,
            }) => {}
            _ => panic!("Must return collateral and native supply mismatch error"),
        }
    }

    #[test]
    fn invalid_trade_collateral_and_fund_mismatch() {
        let marker = create_marker(
            "tp1kn7phy33x5pqpax6t9n60tkjtuqf5jt37txe0h",
            "denom1",
            vec![Coin::new(1000, "denom1"), Coin::new(1000, "denom2")],
            1000,
        );
        let mut deps = mock_dependencies_with_balances(&[(marker.address.as_str(), &marker.coins)]);
        deps.querier.with_markers(vec![marker.clone()]);
        let msg = InstantiateMsg {
            native_denom: marker.denom.clone(),
            collateral_denom: "denom2".to_string(),
            marker_address: marker.address.to_string(),
        };
        let info = mock_info("tp1w9fnesmguvlal3mp62na3f58zww9jtmtwfnx9h", &[]);
        let _ = instantiate(deps.as_mut(), mock_env(), info, msg).unwrap();

        deps.querier.base.update_balance(
            "tp1kn7phy33x5pqpax6t9n60tkjtuqf5jt37txe0h",
            vec![Coin::new(1200, "denom1")],
        );
        let info = mock_info(
            "tp1w9fnesmguvlal3mp62na3f58zww9jtmtwfnx9h",
            &[Coin::new(200, "denom2")],
        );
        let msg = ExecuteMsg::Trade {};

        let res = execute(deps.as_mut(), mock_env(), info, msg);

        match res {
            Err(ContractError::CollateralAndNativeSupplyMistmatchError {
                collateral_denom: _,
                native_denom: _,
                marker_address: _,
            }) => {}
            _ => panic!("Must return collateral and native supply mismatch error"),
        }
    }

    #[test]
    fn invalid_trade_funds_is_empty() {
        let marker = create_marker(
            "tp1kn7phy33x5pqpax6t9n60tkjtuqf5jt37txe0h",
            "denom1",
            vec![Coin::new(1000, "denom1"), Coin::new(1000, "denom2")],
            1000,
        );
        let mut deps = mock_dependencies_with_balances(&[(marker.address.as_str(), &marker.coins)]);
        deps.querier.with_markers(vec![marker.clone()]);
        let msg = InstantiateMsg {
            native_denom: marker.denom.clone(),
            collateral_denom: "denom2".to_string(),
            marker_address: marker.address.to_string(),
        };
        let info = mock_info("tp1w9fnesmguvlal3mp62na3f58zww9jtmtwfnx9h", &[]);
        let _ = instantiate(deps.as_mut(), mock_env(), info, msg).unwrap();

        let info = mock_info("tp1w9fnesmguvlal3mp62na3f58zww9jtmtwfnx9h", &[]);
        let msg = ExecuteMsg::Trade {};

        let res = execute(deps.as_mut(), mock_env(), info, msg);

        match res {
            Err(ContractError::InvalidFundsLengthError {}) => {}
            _ => panic!("Must return invalid funds length error"),
        }
    }

    #[test]
    fn invalid_trade_funds_has_more_than_one_coin() {
        let marker = create_marker(
            "tp1kn7phy33x5pqpax6t9n60tkjtuqf5jt37txe0h",
            "denom1",
            vec![Coin::new(1000, "denom1"), Coin::new(1000, "denom2")],
            1000,
        );
        let mut deps = mock_dependencies_with_balances(&[(marker.address.as_str(), &marker.coins)]);
        deps.querier.with_markers(vec![marker.clone()]);
        let msg = InstantiateMsg {
            native_denom: marker.denom.clone(),
            collateral_denom: "denom2".to_string(),
            marker_address: marker.address.to_string(),
        };
        let info = mock_info("tp1w9fnesmguvlal3mp62na3f58zww9jtmtwfnx9h", &[]);
        let _ = instantiate(deps.as_mut(), mock_env(), info, msg).unwrap();

        let info = mock_info(
            "tp1w9fnesmguvlal3mp62na3f58zww9jtmtwfnx9h",
            &[Coin::new(200, "denom2"), Coin::new(200, "denom2")],
        );
        let msg = ExecuteMsg::Trade {};

        let res = execute(deps.as_mut(), mock_env(), info, msg);

        match res {
            Err(ContractError::InvalidFundsLengthError {}) => {}
            _ => panic!("Must return invalid funds length error"),
        }
    }

    #[test]
    fn invalid_trade_funds_denom_does_not_match() {
        let marker = create_marker(
            "tp1kn7phy33x5pqpax6t9n60tkjtuqf5jt37txe0h",
            "denom1",
            vec![Coin::new(1000, "denom1"), Coin::new(1000, "denom2")],
            1000,
        );
        let mut deps = mock_dependencies_with_balances(&[(marker.address.as_str(), &marker.coins)]);
        deps.querier.with_markers(vec![marker.clone()]);
        let msg = InstantiateMsg {
            native_denom: marker.denom.clone(),
            collateral_denom: "denom2".to_string(),
            marker_address: marker.address.to_string(),
        };
        let info = mock_info("tp1w9fnesmguvlal3mp62na3f58zww9jtmtwfnx9h", &[]);
        let _ = instantiate(deps.as_mut(), mock_env(), info, msg).unwrap();

        let info = mock_info(
            "tp1w9fnesmguvlal3mp62na3f58zww9jtmtwfnx9h",
            &[Coin::new(200, "denom3")],
        );
        let msg = ExecuteMsg::Trade {};

        let res = execute(deps.as_mut(), mock_env(), info, msg);

        match res {
            Err(ContractError::InvalidFundsDenomError {}) => {}
            _ => panic!("Must return invalid funds denom error"),
        }
    }

    #[test]
    fn invalid_trade_funds_is_zero() {
        let marker = create_marker(
            "tp1kn7phy33x5pqpax6t9n60tkjtuqf5jt37txe0h",
            "denom1",
            vec![Coin::new(1000, "denom1"), Coin::new(1000, "denom2")],
            1000,
        );
        let mut deps = mock_dependencies_with_balances(&[(marker.address.as_str(), &marker.coins)]);
        deps.querier.with_markers(vec![marker.clone()]);
        let msg = InstantiateMsg {
            native_denom: marker.denom.clone(),
            collateral_denom: "denom2".to_string(),
            marker_address: marker.address.to_string(),
        };
        let info = mock_info("tp1w9fnesmguvlal3mp62na3f58zww9jtmtwfnx9h", &[]);
        let _ = instantiate(deps.as_mut(), mock_env(), info, msg).unwrap();

        let info = mock_info(
            "tp1w9fnesmguvlal3mp62na3f58zww9jtmtwfnx9h",
            &[Coin::new(0, "denom2")],
        );
        let msg = ExecuteMsg::Trade {};

        let res = execute(deps.as_mut(), mock_env(), info, msg);

        match res {
            Err(ContractError::InvalidFundsAmountError {}) => {}
            _ => panic!("Must return invalid funds amount error"),
        }
    }

    #[test]
    fn trade_collateral_for_native() {
        let marker = create_marker(
            "tp1kn7phy33x5pqpax6t9n60tkjtuqf5jt37txe0h",
            "denom1",
            vec![Coin::new(1000, "denom1"), Coin::new(1000, "denom2")],
            1000,
        );
        let mut deps = mock_dependencies_with_balances(&[(marker.address.as_str(), &marker.coins)]);
        deps.querier.with_markers(vec![marker.clone()]);
        let msg = InstantiateMsg {
            native_denom: marker.denom.clone(),
            collateral_denom: "denom2".to_string(),
            marker_address: marker.address.to_string(),
        };
        let info = mock_info("tp1w9fnesmguvlal3mp62na3f58zww9jtmtwfnx9h", &[]);
        let _ = instantiate(deps.as_mut(), mock_env(), info, msg).unwrap();

        let info = mock_info(
            "tp1w9fnesmguvlal3mp62na3f58zww9jtmtwfnx9h",
            &[Coin::new(200, "denom2")],
        );
        let msg = ExecuteMsg::Trade {};

        let res = execute(deps.as_mut(), mock_env(), info, msg).unwrap();
        assert_eq!(3, res.messages.len());
        assert_eq!(4, res.attributes.len());
        assert_eq!(
            Attribute::new("action", "provwasm.contracts.exchange.trade"),
            res.attributes[0]
        );
        assert_eq!(Attribute::new("integration_test", "v1"), res.attributes[1]);
        assert_eq!(
            Attribute::new("sent", Coin::new(200, "denom2").to_string()),
            res.attributes[2]
        );
        assert_eq!(
            Attribute::new("received", Coin::new(200, "denom1").to_string()),
            res.attributes[3]
        );

        let collateral_send = Bank(BankMsg::Send {
            amount: vec![Coin::new(200, "denom2")],
            to_address: marker.address.to_string(),
        });

        // We want to mint native_denom for the marker
        let mint = mint_marker_supply(200, marker.denom.to_string()).unwrap();

        // Give the new native_denom to the sender
        let withdraw = withdraw_coins(
            marker.denom.clone(),
            200 as u128,
            marker.denom.clone(),
            Addr::unchecked("tp1w9fnesmguvlal3mp62na3f58zww9jtmtwfnx9h"),
        )
        .unwrap();

        assert_eq!(collateral_send, res.messages[0].msg);
        assert_eq!(mint, res.messages[1].msg);
        assert_eq!(withdraw, res.messages[2].msg);
    }

    #[test]
    fn trade_native_for_collateral() {
        let marker = create_marker(
            "tp1kn7phy33x5pqpax6t9n60tkjtuqf5jt37txe0h",
            "denom1",
            vec![Coin::new(1000, "denom1"), Coin::new(1000, "denom2")],
            1000,
        );
        let mut deps = mock_dependencies_with_balances(&[(marker.address.as_str(), &marker.coins)]);
        deps.querier.with_markers(vec![marker.clone()]);
        let msg = InstantiateMsg {
            native_denom: marker.denom.clone(),
            collateral_denom: "denom2".to_string(),
            marker_address: marker.address.to_string(),
        };
        let info = mock_info("tp1w9fnesmguvlal3mp62na3f58zww9jtmtwfnx9h", &[]);
        let _ = instantiate(deps.as_mut(), mock_env(), info, msg).unwrap();

        let info = mock_info(
            "tp1w9fnesmguvlal3mp62na3f58zww9jtmtwfnx9h",
            &[Coin::new(200, "denom1")],
        );
        let msg = ExecuteMsg::Trade {};

        let res = execute(deps.as_mut(), mock_env(), info, msg).unwrap();
        assert_eq!(3, res.messages.len());
        assert_eq!(4, res.attributes.len());
        assert_eq!(
            Attribute::new("action", "provwasm.contracts.exchange.trade"),
            res.attributes[0]
        );
        assert_eq!(Attribute::new("integration_test", "v1"), res.attributes[1]);
        assert_eq!(
            Attribute::new("sent", Coin::new(200, "denom1").to_string()),
            res.attributes[2]
        );
        assert_eq!(
            Attribute::new("received", Coin::new(200, "denom2").to_string()),
            res.attributes[3]
        );

        let collateral_send = Bank(BankMsg::Send {
            amount: vec![Coin::new(200, marker.denom.clone())],
            to_address: marker.address.to_string(),
        });

        // We want to mint native_denom for the marker
        let burn = burn_marker_supply(200, marker.denom.to_string()).unwrap();

        // Give the new native_denom to the sender
        let withdraw = withdraw_coins(
            "denom1".to_string(),
            200 as u128,
            "denom2".to_string(),
            Addr::unchecked("tp1w9fnesmguvlal3mp62na3f58zww9jtmtwfnx9h"),
        )
        .unwrap();

        assert_eq!(collateral_send, res.messages[0].msg);
        assert_eq!(burn, res.messages[1].msg);
        assert_eq!(withdraw, res.messages[2].msg);
    }
}
