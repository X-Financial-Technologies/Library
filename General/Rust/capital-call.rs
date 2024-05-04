use cosmwasm_std::StdError;
use cosmwasm_std::{
    entry_point, to_binary, BankMsg, Binary, Deps, DepsMut, Env, MessageInfo, Response, StdResult,
};
use provwasm_std::{mint_marker_supply, withdraw_coins, ProvenanceMsg, ProvenanceQuerier};

use crate::error::ContractError;
use crate::msg::{HandleMsg, InstantiateMsg, MigrateMsg, QueryMsg};
use crate::state::{config, config_read, State, Status};

fn contract_error(err: &str) -> ContractError {
    ContractError::Std(StdError::generic_err(err))
}

// Note, you can use StdResult in some functions where you do not
// make use of the custom errors
#[entry_point]
pub fn instantiate(
    deps: DepsMut,
    _env: Env,
    info: MessageInfo,
    msg: InstantiateMsg,
) -> Result<Response, ContractError> {
    let state = State {
        status: Status::PendingCapital,
        gp: info.sender,
        lp_capital_source: msg.lp_capital_source,
        admin: msg.admin,
        capital: msg.capital,
        shares: msg.shares,
    };
    config(deps.storage).save(&state)?;

    Ok(Response::default())
}

#[entry_point]
pub fn migrate(_: DepsMut, _: Env, _: MigrateMsg) -> Result<Response, ContractError> {
    Ok(Response::default())
}

// And declare a custom Error variant for the ones where you will want to make use of it
#[entry_point]
pub fn execute(
    deps: DepsMut,
    _env: Env,
    info: MessageInfo,
    msg: HandleMsg,
) -> Result<Response<ProvenanceMsg>, ContractError> {
    match msg {
        HandleMsg::Cancel {} => try_cancel(deps, _env, info),
        HandleMsg::CommitCapital {} => try_commit_capital(deps, _env, info),
        HandleMsg::CallCapital {} => try_call_capital(deps, _env, info),
    }
}

pub fn try_commit_capital(
    deps: DepsMut,
    _env: Env,
    info: MessageInfo,
) -> Result<Response<ProvenanceMsg>, ContractError> {
    let state = config_read(deps.storage).load()?;

    if state.status != Status::PendingCapital {
        return Err(contract_error("contract no longer pending capital"));
    }

    if info.sender != state.lp_capital_source {
        return Err(contract_error("wrong investor committing capital"));
    }

    if info.funds.is_empty() {
        return Err(contract_error("no capital was committed"));
    }

    let deposit = info.funds.first().unwrap();
    if deposit != &state.capital {
        return Err(contract_error("capital does not match required"));
    }

    config(deps.storage).update(|mut state| -> Result<_, ContractError> {
        state.status = Status::CapitalCommitted;
        Ok(state)
    })?;

    Ok(Response::default())
}

pub fn try_cancel(
    deps: DepsMut,
    _env: Env,
    info: MessageInfo,
) -> Result<Response<ProvenanceMsg>, ContractError> {
    let state = config_read(deps.storage).load()?;

    if state.status == Status::CapitalCalled {
        return Err(contract_error("capital already called"));
    } else if state.status == Status::Cancelled {
        return Err(contract_error("already cancelled"));
    }

    if info.sender != state.gp && info.sender != state.admin {
        return Err(contract_error("wrong gp cancelling capital call"));
    }

    config(deps.storage).update(|mut state| -> Result<_, ContractError> {
        state.status = Status::Cancelled;
        Ok(state)
    })?;

    Ok(
        Response::new().add_messages(if state.status == Status::CapitalCommitted {
            vec![BankMsg::Send {
                to_address: state.lp_capital_source.to_string(),
                amount: vec![state.capital],
            }]
        } else {
            vec![]
        }),
    )
}

pub fn try_call_capital(
    deps: DepsMut,
    _env: Env,
    info: MessageInfo,
) -> Result<Response<ProvenanceMsg>, ContractError> {
    let state = config_read(deps.storage).load()?;

    if state.status != Status::CapitalCommitted {
        return Err(contract_error("capital not committed"));
    }

    if info.sender != state.gp && info.sender != state.admin {
        return Err(contract_error("wrong gp calling capital"));
    }

    config(deps.storage).update(|mut state| -> Result<_, ContractError> {
        state.status = Status::CapitalCalled;
        Ok(state)
    })?;

    let mint = mint_marker_supply(state.shares.amount.into(), state.shares.denom.clone())?;
    let withdraw = withdraw_coins(
        state.shares.denom.clone(),
        state.shares.amount.into(),
        state.shares.denom.clone(),
        state.lp_capital_source,
    )?;

    let marker = ProvenanceQuerier::new(&deps.querier).get_marker_by_denom(state.shares.denom)?;

    Ok(Response::new().add_messages(vec![
        mint,
        withdraw,
        BankMsg::Send {
            to_address: marker.address.to_string(),
            amount: vec![state.capital],
        }
        .into(),
    ]))
}

#[entry_point]
pub fn query(deps: Deps, _env: Env, msg: QueryMsg) -> StdResult<Binary> {
    match msg {
        QueryMsg::GetStatus {} => to_binary(&query_status(deps)?),
    }
}

fn query_status(deps: Deps) -> StdResult<Status> {
    let state = config_read(deps.storage).load()?;
    Ok(state.status)
}

#[cfg(test)]
mod tests {
    use super::*;
    use cosmwasm_std::testing::{mock_env, mock_info};
    use cosmwasm_std::{coins, from_binary, Addr, Coin, CosmosMsg};
    use provwasm_mocks::{mock_dependencies, must_read_binary_file};
    use provwasm_std::{Marker, MarkerMsgParams, ProvenanceMsgParams};

    fn inst_msg() -> InstantiateMsg {
        InstantiateMsg {
            lp_capital_source: Addr::unchecked("tp18lysxk7sueunnspju4dar34vlv98a7kyyfkqs7"),
            admin: Addr::unchecked("tp1apnhcu9x5cz2l8hhgnj0hg7ez53jah7hcan000"),
            capital: Coin::new(1000000, "cfigure"),
            shares: Coin::new(10, "fund-coin"),
        }
    }

    #[test]
    fn initialization() {
        let mut deps = mock_dependencies(&[]);
        let info = mock_info("creator", &[]);

        // we can just call .unwrap() to assert this was a success
        let res = instantiate(deps.as_mut(), mock_env(), info, inst_msg()).unwrap();
        assert_eq!(0, res.messages.len());

        // it worked, let's query the state
        let res = query(deps.as_ref(), mock_env(), QueryMsg::GetStatus {}).unwrap();
        let status: Status = from_binary(&res).unwrap();
        assert_eq!(Status::PendingCapital, status);
    }

    #[test]
    fn commit_capital() {
        let mut deps = mock_dependencies(&coins(2, "token"));

        let info = mock_info("creator", &[]);
        let _res = instantiate(deps.as_mut(), mock_env(), info, inst_msg()).unwrap();

        // lp can commit capital
        let info = mock_info(
            "tp18lysxk7sueunnspju4dar34vlv98a7kyyfkqs7",
            &coins(1000000, "cfigure"),
        );
        let msg = HandleMsg::CommitCapital {};
        let _res = execute(deps.as_mut(), mock_env(), info, msg).unwrap();

        // should be in capital commited state
        let res = query(deps.as_ref(), mock_env(), QueryMsg::GetStatus {}).unwrap();
        let status: Status = from_binary(&res).unwrap();
        assert_eq!(Status::CapitalCommitted, status);
    }

    #[test]
    fn cancel() {
        let mut deps = mock_dependencies(&coins(2, "token"));

        let info = mock_info("creator", &[]);
        let _res = instantiate(deps.as_mut(), mock_env(), info, inst_msg()).unwrap();

        // lp can commit capital
        let info = mock_info(
            "tp18lysxk7sueunnspju4dar34vlv98a7kyyfkqs7",
            &coins(1000000, "cfigure"),
        );
        let msg = HandleMsg::CommitCapital {};
        let _res = execute(deps.as_mut(), mock_env(), info, msg).unwrap();

        // gp can cancel capital call
        let info = mock_info("creator", &[]);
        let msg = HandleMsg::Cancel {};
        let _res = execute(deps.as_mut(), mock_env(), info, msg).unwrap();

        // should be in pending capital state
        let res = query(deps.as_ref(), mock_env(), QueryMsg::GetStatus {}).unwrap();
        let status: Status = from_binary(&res).unwrap();
        assert_eq!(Status::Cancelled, status);

        // should send stable coin back to lp
        let (to_address, amount) = _res
            .messages
            .iter()
            .find_map(|sub_msg| match &sub_msg.msg {
                CosmosMsg::Bank(bank) => match bank {
                    BankMsg::Send { to_address, amount } => Some((to_address, amount)),
                    _ => None,
                },
                _ => None,
            })
            .unwrap();
        assert_eq!("tp18lysxk7sueunnspju4dar34vlv98a7kyyfkqs7", to_address);
        assert_eq!(1000000, u128::from(amount[0].amount));
        assert_eq!("cfigure", amount[0].denom);
    }

    #[test]
    fn call_capital() {
        // Create a mock querier with our expected marker.
        let bin = must_read_binary_file("testdata/marker.json");
        let expected_marker: Marker = from_binary(&bin).unwrap();
        let mut deps = mock_dependencies(&[]);
        deps.querier.with_markers(vec![expected_marker.clone()]);

        let info = mock_info("creator", &[]);
        let _res = instantiate(deps.as_mut(), mock_env(), info, inst_msg()).unwrap();

        // lp can commit capital
        let info = mock_info(
            "tp18lysxk7sueunnspju4dar34vlv98a7kyyfkqs7",
            &coins(1000000, "cfigure"),
        );
        let msg = HandleMsg::CommitCapital {};
        let _res = execute(deps.as_mut(), mock_env(), info, msg).unwrap();

        // gp can call capital
        let info = mock_info("creator", &vec![]);
        let msg = HandleMsg::CallCapital {};
        let _res = execute(deps.as_mut(), mock_env(), info, msg).unwrap();

        let mint = _res
            .messages
            .iter()
            .find_map(|sub_msg| match &sub_msg.msg {
                CosmosMsg::Custom(custom) => match custom {
                    ProvenanceMsg {
                        route: _,
                        params,
                        version: _,
                    } => match params {
                        ProvenanceMsgParams::Marker(params) => match params {
                            MarkerMsgParams::MintMarkerSupply { coin } => Some(coin),
                            _ => None,
                        },
                        _ => None,
                    },
                },
                _ => None,
            })
            .unwrap();
        assert_eq!(10, u128::from(mint.amount));

        let (withdraw_coin, withdraw_recipient) = _res
            .messages
            .iter()
            .find_map(|sub_msg| match &sub_msg.msg {
                CosmosMsg::Custom(custom) => match custom {
                    ProvenanceMsg {
                        route: _,
                        params,
                        version: _,
                    } => match params {
                        ProvenanceMsgParams::Marker(params) => match params {
                            MarkerMsgParams::WithdrawCoins {
                                marker_denom: _,
                                coin,
                                recipient,
                            } => Some((coin, recipient)),
                            _ => None,
                        },
                        _ => None,
                    },
                },
                _ => None,
            })
            .unwrap();
        assert_eq!(10, u128::from(withdraw_coin.amount));
        assert_eq!(
            "tp18lysxk7sueunnspju4dar34vlv98a7kyyfkqs7",
            withdraw_recipient.to_string()
        );

        let (to_address, amount) = _res
            .messages
            .iter()
            .find_map(|sub_msg| match &sub_msg.msg {
                CosmosMsg::Bank(bank) => match bank {
                    BankMsg::Send { to_address, amount } => Some((to_address, amount)),
                    _ => None,
                },
                _ => None,
            })
            .unwrap();
        assert_eq!("tp18vmzryrvwaeykmdtu6cfrz5sau3dhc5c73ms0u", to_address);
        assert_eq!(1000000, u128::from(amount[0].amount));
        assert_eq!("cfigure", amount[0].denom);

        // should be in capital called state
        let res = query(deps.as_ref(), mock_env(), QueryMsg::GetStatus {}).unwrap();
        let status: Status = from_binary(&res).unwrap();
        assert_eq!(Status::CapitalCalled, status);
    }
}
