use std::convert::TryFrom;
use std::fmt;

use cosmwasm_std::{
    attr, to_binary, Binary, Deps, DepsMut, Empty, Env, MessageInfo, Response, StdError, StdResult,
    Uint128,
};
use cosmwasm_std::{entry_point, Addr};
use provwasm_std::types::cosmos::base::v1beta1::Coin;
use provwasm_std::types::provenance::marker::v1::{
    Access, MarkerAccount, MarkerQuerier, MsgTransferRequest,
};

use crate::error::ContractError;
use crate::msg::{ExecuteMsg, QueryMsg, Validate};
use crate::state::{get_all_transfers, Transfer, CONFIG, TRANSFER_STORAGE};

pub const CRATE_NAME: &str = env!("CARGO_CRATE_NAME");
pub const PACKAGE_VERSION: &str = env!("CARGO_PKG_VERSION");

// smart contract execute entrypoint
#[entry_point]
pub fn execute(
    deps: DepsMut,
    env: Env,
    info: MessageInfo,
    msg: ExecuteMsg,
) -> Result<Response, ContractError> {
    msg.validate()?;

    match msg {
        ExecuteMsg::ApproveTransfer { id } => approve_transfer(deps, env, info, id),
        ExecuteMsg::CancelTransfer { id } => cancel_transfer(deps, env, info, id),
        ExecuteMsg::RejectTransfer { id } => reject_transfer(deps, env, info, id),
        ExecuteMsg::Transfer {
            id,
            denom,
            amount,
            recipient,
        } => create_transfer(deps, env, info, id, denom, amount, recipient),
    }
}

fn create_transfer(
    deps: DepsMut,
    env: Env,
    info: MessageInfo,
    id: String,
    denom: String,
    amount: Uint128,
    recipient: String,
) -> Result<Response, ContractError> {
    let transfer = Transfer {
        id,
        sender: info.sender.to_owned(),
        denom,
        amount,
        recipient: deps.api.addr_validate(&recipient)?,
    };

    let querier = MarkerQuerier::new(&deps.querier);

    let is_restricted_marker = matches!(
        get_marker_by_denom(transfer.denom.clone(), &querier),
        Ok(MarkerAccount {
            marker_type: 2, // MarkerType::Restricted,
            ..
        })
    );

    match is_restricted_marker {
        // funds should not be sent
        true => {
            if !info.funds.is_empty() {
                return Err(ContractError::SentFundsUnsupported);
            }
        }
        false => {
            return Err(ContractError::UnsupportedMarkerType);
        }
    }

    // Ensure the sender holds enough denom to cover the transfer.
    let balance = deps
        .querier
        .query_balance(info.sender.clone(), transfer.denom.clone())?;

    if balance.amount < transfer.amount {
        return Err(ContractError::InsufficientFunds);
    }

    if TRANSFER_STORAGE
        .may_load(deps.storage, transfer.id.as_bytes())?
        .is_some()
    {
        return Err(ContractError::InvalidFields {
            fields: vec![String::from("id")],
        });
    }

    TRANSFER_STORAGE.save(deps.storage, transfer.id.as_bytes(), &transfer)?;

    let mut response = Response::new().add_attributes(vec![
        attr("action", Action::Transfer.to_string()),
        attr("id", &transfer.id),
        attr("denom", &transfer.denom),
        attr("amount", transfer.amount.to_string()),
        attr("sender", &transfer.sender),
        attr("recipient", &transfer.recipient),
    ]);

    let coin = Coin {
        denom: transfer.denom.to_owned(),
        amount: transfer.amount.into(),
    };

    response = response.add_message(MsgTransferRequest {
        amount: Some(coin),
        to_address: env.contract.address.to_string(),
        from_address: transfer.sender.to_string(),
        administrator: env.contract.address.to_string(),
    });

    Ok(response)
}

pub fn cancel_transfer(
    deps: DepsMut,
    env: Env,
    info: MessageInfo,
    transfer_id: String,
) -> Result<Response, ContractError> {
    let transfer = TRANSFER_STORAGE
        .load(deps.storage, transfer_id.as_bytes())
        .map_err(|error| ContractError::LoadTransferFailed { error })?;

    if !info.funds.is_empty() {
        return Err(ContractError::SentFundsUnsupported);
    }

    if !info.sender.eq(&transfer.sender) {
        return Err(ContractError::Unauthorized {
            error: String::from("Only original sender can cancel"),
        });
    }

    let mut response = Response::new().add_attributes(vec![
        attr("action", Action::Cancel.to_string()),
        attr("id", &transfer.id),
        attr("denom", &transfer.denom),
        attr("amount", transfer.amount.to_string()),
        attr("sender", &transfer.sender),
    ]);

    let coin = Coin {
        denom: transfer.denom.to_owned(),
        amount: transfer.amount.into(),
    };

    response = response.add_message(MsgTransferRequest {
        amount: Some(coin),
        to_address: transfer.sender.to_string(),
        from_address: env.contract.address.to_string(),
        administrator: env.contract.address.to_string(),
    });

    // finally remove the transfer from storage
    TRANSFER_STORAGE.remove(deps.storage, transfer_id.as_bytes());

    Ok(response)
}

pub fn reject_transfer(
    deps: DepsMut,
    env: Env,
    info: MessageInfo,
    transfer_id: String,
) -> Result<Response, ContractError> {
    let transfer = TRANSFER_STORAGE
        .load(deps.storage, transfer_id.as_bytes())
        .map_err(|error| ContractError::LoadTransferFailed { error })?;

    if !info.funds.is_empty() {
        return Err(ContractError::SentFundsUnsupported);
    }

    let querier = MarkerQuerier::new(&deps.querier);
    let marker = get_marker_by_denom(transfer.denom.clone(), &querier)?;

    if !has_marker_access_transfer(info.sender.to_owned(), marker) {
        return Err(ContractError::Unauthorized {
            error: String::from("ACCESS_TRANSFER permission is required to reject transfers"),
        });
    }

    let mut response = Response::new().add_attributes(vec![
        attr("action", Action::Reject.to_string()),
        attr("id", &transfer.id),
        attr("denom", &transfer.denom),
        attr("amount", transfer.amount.to_string()),
        attr("sender", &transfer.sender),
        attr("admin", info.sender.to_owned()),
    ]);

    let coin = Coin {
        denom: transfer.denom.to_owned(),
        amount: transfer.amount.into(),
    };

    response = response.add_message(MsgTransferRequest {
        amount: Some(coin),
        to_address: transfer.sender.to_string(),
        from_address: env.contract.address.to_string(),
        administrator: env.contract.address.to_string(),
    });

    // finally remove the transfer from storage
    TRANSFER_STORAGE.remove(deps.storage, transfer_id.as_bytes());

    Ok(response)
}

pub fn approve_transfer(
    deps: DepsMut,
    env: Env,
    info: MessageInfo,
    transfer_id: String,
) -> Result<Response, ContractError> {
    let transfer = TRANSFER_STORAGE
        .load(deps.storage, transfer_id.as_bytes())
        .map_err(|error| ContractError::LoadTransferFailed { error })?;

    if !info.funds.is_empty() {
        return Err(ContractError::SentFundsUnsupported);
    }

    let querier = MarkerQuerier::new(&deps.querier);
    let marker = get_marker_by_denom(transfer.denom.clone(), &querier)?;

    if !has_marker_access_transfer(info.sender.to_owned(), marker) {
        return Err(ContractError::Unauthorized {
            error: String::from("ACCESS_TRANSFER permission is required to approve transfers"),
        });
    }

    let mut response = Response::new().add_attributes(vec![
        attr("action", Action::Approve.to_string()),
        attr("id", &transfer.id),
        attr("denom", &transfer.denom),
        attr("amount", transfer.amount.to_string()),
        attr("sender", &transfer.sender),
        attr("recipient", &transfer.recipient),
        attr("admin", &info.sender),
    ]);

    let coin = Coin {
        denom: transfer.denom.to_owned(),
        amount: transfer.amount.into(),
    };

    response = response.add_message(MsgTransferRequest {
        amount: Some(coin),
        to_address: transfer.recipient.to_owned().to_string(),
        from_address: env.contract.address.to_string(),
        administrator: env.contract.address.to_string(),
    });

    // finally remove the transfer from storage
    TRANSFER_STORAGE.remove(deps.storage, transfer_id.as_bytes());
    Ok(response)
}

/// returns true if the sender has marker transfer permissions for the given marker
fn has_marker_access_transfer(sender: Addr, marker: MarkerAccount) -> bool {
    let access_transfer: i32 = Access::Transfer.into();
    marker.access_control.iter().any(|grant| {
        grant.address == sender
            && grant
                .permissions
                .iter()
                .any(|marker_access| *marker_access == access_transfer)
    })
}

fn get_marker_by_denom(denom: String, querier: &MarkerQuerier<Empty>) -> StdResult<MarkerAccount> {
    let response = querier.marker(denom)?;
    if let Some(marker) = response.marker {
        return if let Ok(account) = MarkerAccount::try_from(marker) {
            Ok(account)
        } else {
            Err(StdError::generic_err("unable to type-cast marker account"))
        };
    }
    Err(StdError::generic_err("no marker found for denom"))
}

#[entry_point]
pub fn query(deps: Deps, _env: Env, msg: QueryMsg) -> StdResult<Binary> {
    msg.validate()?;

    match msg {
        QueryMsg::GetContractInfo {} => to_binary(&CONFIG.load(deps.storage)?),
        QueryMsg::GetVersionInfo {} => to_binary(&cw2::get_contract_version(deps.storage)?),
        QueryMsg::GetTransfer { id: transfer_id } => {
            to_binary(&TRANSFER_STORAGE.load(deps.storage, transfer_id.as_bytes())?)
        }
        QueryMsg::GetAllTransfers {} => to_binary(&get_all_transfers(deps.storage)),
    }
}

enum Action {
    Transfer,
    Approve,
    Reject,
    Cancel,
}

impl fmt::Display for Action {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Action::Transfer => write!(f, "create_transfer"),
            Action::Approve => write!(f, "approve"),
            Action::Reject => write!(f, "reject"),
            Action::Cancel => write!(f, "cancel"),
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::state::{State, CONFIG};
    use cosmwasm_std::testing::{mock_env, mock_info, MOCK_CONTRACT_ADDR};
    use cosmwasm_std::{coin, from_binary, Addr, CosmosMsg, Storage};
    use prost::Message;
    use provwasm_mocks::{mock_provenance_dependencies, MockProvenanceQuerier};
    use provwasm_std::shim::Any;
    use provwasm_std::types::cosmos::auth::v1beta1::BaseAccount;
    use provwasm_std::types::provenance::marker::v1::{
        Access, AccessGrant, MarkerStatus, MarkerType, QueryMarkerRequest, QueryMarkerResponse,
    };
    use std::convert::TryInto;

    use super::*;

    const RESTRICTED_DENOM: &str = "restricted_1";
    const TRANSFER_ID: &str = "56253028-12f5-4d2a-a691-ebdfd2a7b865";

    #[test]
    fn create_transfer_success() {
        let mut deps = mock_provenance_dependencies();
        setup_test_base(
            &mut deps.storage,
            &State {
                name: "contract_name".into(),
            },
        );

        let test_marker: MarkerAccount = setup_restricted_marker();
        mock_query_marker_response(&test_marker, &mut deps.querier);

        let amount = Uint128::new(1);
        let transfer_msg = ExecuteMsg::Transfer {
            id: TRANSFER_ID.into(),
            denom: RESTRICTED_DENOM.into(),
            amount: amount.into(),
            recipient: "transfer_to".into(),
        };

        let sender_info = mock_info("sender", &[]);

        let sender_balance = coin(1, RESTRICTED_DENOM);
        deps.querier
            .mock_querier
            .update_balance(Addr::unchecked("sender"), vec![sender_balance]);

        let recipient = "transfer_to";

        // execute create transfer
        let transfer_response = execute(
            deps.as_mut(),
            mock_env(),
            sender_info.clone(),
            transfer_msg.clone(),
        );

        let expected_coin = Coin {
            denom: RESTRICTED_DENOM.to_owned(),
            amount: amount.into(),
        };

        // verify transfer response
        match transfer_response {
            Ok(response) => {
                assert_eq!(response.attributes.len(), 6);
                assert_eq!(
                    response.attributes[0],
                    attr("action", Action::Transfer.to_string())
                );
                assert_eq!(response.attributes[1], attr("id", TRANSFER_ID));
                assert_eq!(response.attributes[2], attr("denom", RESTRICTED_DENOM));
                assert_eq!(response.attributes[3], attr("amount", amount.to_string()));
                assert_eq!(
                    response.attributes[4],
                    attr("sender", sender_info.clone().sender)
                );
                assert_eq!(response.attributes[5], attr("recipient", recipient));

                assert_eq!(response.messages.len(), 1);

                let expected_message: Binary = MsgTransferRequest {
                    amount: Some(expected_coin),
                    from_address: sender_info.clone().sender.to_string(),
                    to_address: MOCK_CONTRACT_ADDR.to_owned(),
                    administrator: MOCK_CONTRACT_ADDR.to_owned(),
                }
                .try_into()
                .unwrap();

                match &response.messages[0].msg {
                    CosmosMsg::Stargate { type_url, value } => {
                        assert_eq!(type_url, "/provenance.marker.v1.MsgTransferRequest");
                        assert_eq!(value, &expected_message);
                    }
                    _ => panic!("unexpected cosmos message"),
                }
            }
            Err(error) => {
                panic!("failed to create transfer: {:?}", error)
            }
        }

        // verify transfer stored
        match TRANSFER_STORAGE.load(&deps.storage, TRANSFER_ID.as_bytes()) {
            Ok(stored_transfer) => {
                assert_eq!(
                    stored_transfer,
                    Transfer {
                        id: TRANSFER_ID.into(),
                        sender: sender_info.sender.to_owned(),
                        denom: RESTRICTED_DENOM.into(),
                        amount,
                        recipient: Addr::unchecked(recipient)
                    }
                )
            }
            _ => {
                panic!("transfer was not found in storage")
            }
        }
    }

    #[test]
    fn create_transfer_with_funds_throws_error() {
        let mut deps = mock_provenance_dependencies();
        setup_test_base(
            &mut deps.storage,
            &State {
                name: "contract_name".into(),
            },
        );

        let test_marker: MarkerAccount = setup_restricted_marker();
        mock_query_marker_response(&test_marker, &mut deps.querier);

        let amount = Uint128::new(1);
        let transfer_msg = ExecuteMsg::Transfer {
            id: "56253028-12f5-4d2a-a691-ebdfd2a7b865".into(),
            denom: RESTRICTED_DENOM.into(),
            amount: amount.into(),
            recipient: "transfer_to".into(),
        };

        let sender_info = mock_info("sender", &[coin(amount.u128(), RESTRICTED_DENOM)]);

        let sender_balance = coin(1, RESTRICTED_DENOM);
        deps.querier
            .mock_querier
            .update_balance(Addr::unchecked("sender"), vec![sender_balance]);

        // execute create transfer
        let transfer_response = execute(
            deps.as_mut(),
            mock_env(),
            sender_info.clone(),
            transfer_msg.clone(),
        );

        assert_sent_funds_unsupported_error(transfer_response);
    }

    #[test]
    fn create_transfer_insufficient_funds_throws_error() {
        let mut deps = mock_provenance_dependencies();
        setup_test_base(
            &mut deps.storage,
            &State {
                name: "contract_name".into(),
            },
        );

        let test_marker: MarkerAccount = setup_restricted_marker();
        mock_query_marker_response(&test_marker, &mut deps.querier);

        let amount = Uint128::new(2);
        let transfer_msg = ExecuteMsg::Transfer {
            id: TRANSFER_ID.into(),
            denom: RESTRICTED_DENOM.into(),
            amount: amount.into(),
            recipient: "transfer_to".into(),
        };

        let sender_info = mock_info("sender", &[]);

        let sender_balance = coin(1, RESTRICTED_DENOM);
        deps.querier
            .mock_querier
            .update_balance(Addr::unchecked("sender"), vec![sender_balance]);

        // execute create transfer
        let transfer_response = execute(
            deps.as_mut(),
            mock_env(),
            sender_info.clone(),
            transfer_msg.clone(),
        );

        // verify transfer response
        match transfer_response {
            Ok(..) => {
                panic!("expected error, but ok")
            }
            Err(error) => match error {
                ContractError::InsufficientFunds => {}
                error => panic!("unexpected error: {:?}", error),
            },
        }
    }

    #[test]
    fn create_transfer_invalid_data() {
        let mut deps = mock_provenance_dependencies();
        setup_test_base(
            &mut deps.storage,
            &State {
                name: "contract_name".into(),
            },
        );

        let test_marker: MarkerAccount = setup_restricted_marker();
        mock_query_marker_response(&test_marker, &mut deps.querier);

        let amount = Uint128::new(1);
        let transfer_msg = ExecuteMsg::Transfer {
            id: "".into(),
            denom: RESTRICTED_DENOM.into(),
            amount: amount.into(),
            recipient: "transfer_to".into(),
        };

        let sender_info = mock_info("sender", &[]);

        let sender_balance = coin(1, RESTRICTED_DENOM);
        deps.querier
            .mock_querier
            .update_balance(Addr::unchecked("sender"), vec![sender_balance]);

        // execute create transfer
        let transfer_response = execute(
            deps.as_mut(),
            mock_env(),
            sender_info.clone(),
            transfer_msg.clone(),
        );

        // verify transfer response
        match transfer_response {
            Ok(..) => panic!("expected error, but ok"),
            Err(error) => match error {
                ContractError::InvalidFields { fields } => {
                    assert!(fields.contains(&"id".into()));
                }
                error => panic!("unexpected error: {:?}", error),
            },
        }
    }

    #[test]
    fn create_transfer_existing_id() {
        let mut deps = mock_provenance_dependencies();
        setup_test_base(
            &mut deps.storage,
            &State {
                name: "contract_name".into(),
            },
        );

        let test_marker: MarkerAccount = setup_restricted_marker();
        mock_query_marker_response(&test_marker, &mut deps.querier);

        let amount = Uint128::new(1);
        let sender_info = mock_info("sender", &[]);

        store_test_transfer(
            &mut deps.storage,
            &Transfer {
                id: TRANSFER_ID.into(),
                sender: sender_info.sender.to_owned(),
                denom: RESTRICTED_DENOM.into(),
                amount,
                recipient: Addr::unchecked("transfer_to"),
            },
        );

        let transfer_msg = ExecuteMsg::Transfer {
            id: TRANSFER_ID.into(),
            denom: RESTRICTED_DENOM.into(),
            amount: amount.into(),
            recipient: "transfer_to".into(),
        };

        let sender_balance = coin(1, RESTRICTED_DENOM);
        deps.querier
            .mock_querier
            .update_balance(Addr::unchecked("sender"), vec![sender_balance]);

        // execute create transfer
        let transfer_response = execute(
            deps.as_mut(),
            mock_env(),
            sender_info.clone(),
            transfer_msg.clone(),
        );

        // verify transfer response
        match transfer_response {
            Ok(..) => panic!("expected error, but ok"),
            Err(error) => match error {
                ContractError::InvalidFields { fields } => {
                    assert!(fields.contains(&"id".into()));
                }
                error => panic!("unexpected error: {:?}", error),
            },
        }
    }

    #[test]
    fn create_transfer_unrestricted_marker_throws_error() {
        let mut deps = mock_provenance_dependencies();
        setup_test_base(
            &mut deps.storage,
            &State {
                name: "contract_name".into(),
            },
        );

        let amount = Uint128::new(1);
        let transfer_msg = ExecuteMsg::Transfer {
            id: TRANSFER_ID.into(),
            denom: "unrestricted-marker".into(),
            amount: amount.into(),
            recipient: "transfer_to".into(),
        };

        let sender_info = mock_info("sender", &[]);

        let sender_balance = coin(amount.u128(), "unrestricted-marker");
        deps.querier
            .mock_querier
            .update_balance(Addr::unchecked("sender"), vec![sender_balance]);

        // execute create transfer
        let transfer_response = execute(
            deps.as_mut(),
            mock_env(),
            sender_info.clone(),
            transfer_msg.clone(),
        );

        // verify transfer response
        match transfer_response {
            Ok(..) => panic!("expected error, but ok"),
            Err(error) => match error {
                ContractError::UnsupportedMarkerType => {}
                error => panic!("unexpected error: {:?}", error),
            },
        }
    }

    #[test]
    fn approve_transfer_success() {
        let mut deps = mock_provenance_dependencies();
        setup_test_base(
            &mut deps.storage,
            &State {
                name: "contract_name".into(),
            },
        );

        let transfer_address = Addr::unchecked("transfer_address");
        let sender_address = Addr::unchecked("sender_address");
        let recipient_address = Addr::unchecked("transfer_to");

        let test_marker: MarkerAccount =
            setup_restricted_marker_transfer(RESTRICTED_DENOM.into(), transfer_address.to_owned());
        mock_query_marker_response(&test_marker, &mut deps.querier);

        let amount = Uint128::new(1);
        let sender_info = mock_info(transfer_address.as_str(), &[]);

        store_test_transfer(
            &mut deps.storage,
            &Transfer {
                id: TRANSFER_ID.into(),
                sender: sender_address.to_owned(),
                denom: RESTRICTED_DENOM.into(),
                amount,
                recipient: recipient_address.to_owned(),
            },
        );

        let approve_transfer_msg = ExecuteMsg::ApproveTransfer {
            id: TRANSFER_ID.into(),
        };

        // execute approve transfer
        let transfer_response = execute(
            deps.as_mut(),
            mock_env(),
            sender_info.clone(),
            approve_transfer_msg.clone(),
        );

        let expected_coin = Coin {
            denom: RESTRICTED_DENOM.to_owned(),
            amount: amount.into(),
        };

        // verify approve transfer response
        match transfer_response {
            Ok(response) => {
                assert_eq!(response.attributes.len(), 7);
                assert_eq!(
                    response.attributes[0],
                    attr("action", Action::Approve.to_string())
                );
                assert_eq!(response.attributes[1], attr("id", TRANSFER_ID));
                assert_eq!(response.attributes[2], attr("denom", RESTRICTED_DENOM));
                assert_eq!(response.attributes[3], attr("amount", amount.to_string()));
                assert_eq!(response.attributes[4], attr("sender", sender_address));
                assert_eq!(
                    response.attributes[5],
                    attr("recipient", recipient_address.to_owned())
                );
                assert_eq!(response.attributes[6], attr("admin", transfer_address));

                assert_eq!(response.messages.len(), 1);

                let expected_message: Binary = MsgTransferRequest {
                    amount: Some(expected_coin),
                    from_address: MOCK_CONTRACT_ADDR.to_owned(),
                    to_address: recipient_address.to_string(),
                    administrator: MOCK_CONTRACT_ADDR.to_owned(),
                }
                .try_into()
                .unwrap();

                match &response.messages[0].msg {
                    CosmosMsg::Stargate { type_url, value } => {
                        assert_eq!(type_url, "/provenance.marker.v1.MsgTransferRequest");
                        assert_eq!(value, &expected_message);
                    }
                    _ => panic!("unexpected cosmos message"),
                }
            }
            Err(error) => {
                panic!("failed to create transfer: {:?}", error)
            }
        }

        assert_eq!(
            None,
            TRANSFER_STORAGE
                .may_load(&deps.storage, TRANSFER_ID.as_bytes())
                .unwrap()
        );
    }

    #[test]
    fn approve_transfer_sent_funds_returns_error() {
        let mut deps = mock_provenance_dependencies();
        setup_test_base(
            &mut deps.storage,
            &State {
                name: "contract_name".into(),
            },
        );

        let transfer_address = Addr::unchecked("transfer_address");
        let sender_address = Addr::unchecked("sender_address");
        let recipient_address = Addr::unchecked("transfer_to");

        let test_marker: MarkerAccount =
            setup_restricted_marker_transfer(RESTRICTED_DENOM.into(), transfer_address.to_owned());
        mock_query_marker_response(&test_marker, &mut deps.querier);

        let amount = Uint128::new(1);
        let sender_info = mock_info(transfer_address.as_str(), &[coin(1, RESTRICTED_DENOM)]);

        let stored_transfer = Transfer {
            id: TRANSFER_ID.into(),
            sender: sender_address.to_owned(),
            denom: RESTRICTED_DENOM.into(),
            amount,
            recipient: recipient_address.to_owned(),
        };
        store_test_transfer(&mut deps.storage, &stored_transfer);

        let approve_transfer_msg = ExecuteMsg::ApproveTransfer {
            id: TRANSFER_ID.into(),
        };

        // execute approve transfer
        let transfer_response = execute(
            deps.as_mut(),
            mock_env(),
            sender_info.clone(),
            approve_transfer_msg.clone(),
        );

        // verify approve transfer response
        assert_sent_funds_unsupported_error(transfer_response);

        assert_eq!(
            stored_transfer,
            TRANSFER_STORAGE
                .load(&deps.storage, TRANSFER_ID.as_bytes())
                .unwrap()
        );
    }

    #[test]
    fn approve_transfer_unauthorized() {
        let mut deps = mock_provenance_dependencies();
        setup_test_base(
            &mut deps.storage,
            &State {
                name: "contract_name".into(),
            },
        );

        let transfer_address = Addr::unchecked("transfer_address");
        let approver_address = Addr::unchecked("approver_address");
        let sender_address = Addr::unchecked("sender_address");
        let recipient_address = Addr::unchecked("transfer_to");

        let test_marker: MarkerAccount =
            setup_restricted_marker_transfer(RESTRICTED_DENOM.into(), transfer_address.to_owned());
        mock_query_marker_response(&test_marker, &mut deps.querier);

        let amount = Uint128::new(1);
        let sender_info = mock_info(approver_address.as_str(), &[]);

        let stored_transfer = Transfer {
            id: TRANSFER_ID.into(),
            sender: sender_address.to_owned(),
            denom: RESTRICTED_DENOM.into(),
            amount,
            recipient: recipient_address.to_owned(),
        };
        store_test_transfer(&mut deps.storage, &stored_transfer);

        let approve_transfer_msg = ExecuteMsg::ApproveTransfer {
            id: TRANSFER_ID.into(),
        };

        // execute approve transfer
        let transfer_response = execute(
            deps.as_mut(),
            mock_env(),
            sender_info.clone(),
            approve_transfer_msg.clone(),
        );

        match transfer_response {
            Ok(..) => {
                panic!("expected error, but ok")
            }
            Err(error) => match error {
                ContractError::Unauthorized { .. } => {}
                error => panic!("unexpected error: {:?}", error),
            },
        }

        assert_eq!(
            stored_transfer,
            TRANSFER_STORAGE
                .load(&deps.storage, TRANSFER_ID.as_bytes())
                .unwrap()
        );
    }

    #[test]
    fn approve_transfer_unknown_transfer() {
        let mut deps = mock_provenance_dependencies();
        setup_test_base(
            &mut deps.storage,
            &State {
                name: "contract_name".into(),
            },
        );

        let transfer_address = Addr::unchecked("transfer_address");
        let sender_info = mock_info(transfer_address.as_str(), &[]);

        let approve_transfer_msg = ExecuteMsg::ApproveTransfer {
            id: TRANSFER_ID.into(),
        };

        // execute approve transfer
        let transfer_response = execute(
            deps.as_mut(),
            mock_env(),
            sender_info.clone(),
            approve_transfer_msg.clone(),
        );

        assert_load_transfer_error(transfer_response);
    }

    #[test]
    fn has_marker_access_transfer_success() {
        let transfer_address = Addr::unchecked("transfer_address");
        let test_marker: MarkerAccount =
            setup_restricted_marker_transfer(RESTRICTED_DENOM.into(), transfer_address.to_owned());
        assert!(has_marker_access_transfer(
            transfer_address.to_owned(),
            test_marker.into()
        ))
    }

    #[test]
    fn has_marker_access_transfer_returns_false_with_no_permission() {
        let transfer_address = Addr::unchecked("transfer_address");
        let other_address = Addr::unchecked("other_address");
        let test_marker: MarkerAccount =
            setup_restricted_marker_transfer(RESTRICTED_DENOM.into(), transfer_address.to_owned());
        assert_eq!(
            false,
            has_marker_access_transfer(other_address.to_owned(), test_marker.into())
        )
    }

    #[test]
    fn has_marker_access_transfer_returns_false_without_transfer_permission() {
        let non_transfer_address = Addr::unchecked("some_address_without_transfer");
        let test_marker: MarkerAccount = MarkerAccount {
            base_account: Some(BaseAccount {
                address: "tp1l330sxue4suxz9dhc40e2pns0ymrytf8uz4squ".to_string(),
                pub_key: None,
                account_number: 10,
                sequence: 0,
            }),
            manager: "tp13pnzut8zdjaqht7aqe7kk4ww5zfq04jzlytnmu".to_string(),
            access_control: vec![AccessGrant {
                address: "some_address_without_transfer".to_string(),
                permissions: vec![(Access::Admin).into()],
            }],
            status: 0,
            denom: "restricted_1".to_string(),
            supply: "1000".to_string(),
            marker_type: 0,
            supply_fixed: false,
            allow_governance_control: true,
            allow_forced_transfer: false,
            required_attributes: vec![],
        };

        assert_eq!(
            false,
            has_marker_access_transfer(non_transfer_address.to_owned(), test_marker.into())
        )
    }

    #[test]
    fn cancel_transfer_success() {
        let mut deps = mock_provenance_dependencies();
        setup_test_base(
            &mut deps.storage,
            &State {
                name: "contract_name".into(),
            },
        );

        let sender_address = Addr::unchecked("sender_address");
        let recipient_address = Addr::unchecked("transfer_to");

        let amount = Uint128::new(3);
        let sender_info = mock_info(sender_address.as_str(), &[]);

        store_test_transfer(
            &mut deps.storage,
            &Transfer {
                id: TRANSFER_ID.into(),
                sender: sender_address.to_owned(),
                denom: RESTRICTED_DENOM.into(),
                amount,
                recipient: recipient_address.to_owned(),
            },
        );

        let cancel_transfer_msg = ExecuteMsg::CancelTransfer {
            id: TRANSFER_ID.into(),
        };

        // execute cancel transfer
        let cancel_response = execute(
            deps.as_mut(),
            mock_env(),
            sender_info.clone(),
            cancel_transfer_msg.clone(),
        );

        let expected_coin = Coin {
            denom: RESTRICTED_DENOM.to_owned(),
            amount: amount.into(),
        };

        // verify approve transfer response
        match cancel_response {
            Ok(response) => {
                assert_eq!(response.attributes.len(), 5);
                assert_eq!(
                    response.attributes[0],
                    attr("action", Action::Cancel.to_string())
                );
                assert_eq!(response.attributes[1], attr("id", TRANSFER_ID));
                assert_eq!(response.attributes[2], attr("denom", RESTRICTED_DENOM));
                assert_eq!(response.attributes[3], attr("amount", amount.to_string()));
                assert_eq!(
                    response.attributes[4],
                    attr("sender", sender_address.to_owned())
                );

                assert_eq!(response.messages.len(), 1);

                let expected_message: Binary = MsgTransferRequest {
                    amount: Some(expected_coin),
                    from_address: MOCK_CONTRACT_ADDR.to_owned(),
                    to_address: sender_info.clone().sender.to_string(),
                    administrator: MOCK_CONTRACT_ADDR.to_owned(),
                }
                .try_into()
                .unwrap();

                match &response.messages[0].msg {
                    CosmosMsg::Stargate { type_url, value } => {
                        assert_eq!(type_url, "/provenance.marker.v1.MsgTransferRequest");
                        assert_eq!(value, &expected_message);
                    }
                    _ => panic!("unexpected cosmos message"),
                }
            }
            Err(error) => {
                panic!("failed to cancel transfer: {:?}", error)
            }
        }

        assert_eq!(
            None,
            TRANSFER_STORAGE
                .may_load(&deps.storage, TRANSFER_ID.as_bytes())
                .unwrap()
        );
    }

    #[test]
    fn cancel_transfer_sent_funds_returns_error() {
        let mut deps = mock_provenance_dependencies();
        setup_test_base(
            &mut deps.storage,
            &State {
                name: "contract_name".into(),
            },
        );

        let sender_address = Addr::unchecked("sender_address");
        let recipient_address = Addr::unchecked("transfer_to");

        let amount = Uint128::new(3);
        let sender_info = mock_info(sender_address.as_str(), &[coin(1, RESTRICTED_DENOM)]);

        let stored_transfer = Transfer {
            id: TRANSFER_ID.into(),
            sender: sender_address.to_owned(),
            denom: RESTRICTED_DENOM.into(),
            amount,
            recipient: recipient_address.to_owned(),
        };
        store_test_transfer(&mut deps.storage, &stored_transfer);

        let cancel_transfer_msg = ExecuteMsg::CancelTransfer {
            id: TRANSFER_ID.into(),
        };

        // execute cancel transfer
        let transfer_response = execute(
            deps.as_mut(),
            mock_env(),
            sender_info.clone(),
            cancel_transfer_msg.clone(),
        );

        // verify cancel transfer response
        assert_sent_funds_unsupported_error(transfer_response);

        assert_eq!(
            stored_transfer,
            TRANSFER_STORAGE
                .load(&deps.storage, TRANSFER_ID.as_bytes())
                .unwrap()
        );
    }

    #[test]
    fn cancel_transfer_unauthorized() {
        let mut deps = mock_provenance_dependencies();
        setup_test_base(
            &mut deps.storage,
            &State {
                name: "contract_name".into(),
            },
        );

        let sender_address = Addr::unchecked("sender_address");
        let recipient_address = Addr::unchecked("transfer_to");

        let amount = Uint128::new(3);
        let sender_info = mock_info(&"other_address".to_string(), &[]);

        let stored_transfer = Transfer {
            id: TRANSFER_ID.into(),
            sender: sender_address.to_owned(),
            denom: RESTRICTED_DENOM.into(),
            amount,
            recipient: recipient_address.to_owned(),
        };
        store_test_transfer(&mut deps.storage, &stored_transfer);

        let cancel_transfer_msg = ExecuteMsg::CancelTransfer {
            id: TRANSFER_ID.into(),
        };

        // execute cancel transfer
        let transfer_response = execute(
            deps.as_mut(),
            mock_env(),
            sender_info.clone(),
            cancel_transfer_msg.clone(),
        );

        // verify cancel transfer response
        match transfer_response {
            Ok(..) => panic!("expected error, but ok"),
            Err(error) => match error {
                ContractError::Unauthorized { .. } => {}
                error => panic!("unexpected error: {:?}", error),
            },
        }

        assert_eq!(
            stored_transfer,
            TRANSFER_STORAGE
                .load(&deps.storage, TRANSFER_ID.as_bytes())
                .unwrap()
        );
    }

    #[test]
    fn cancel_transfer_unknown_transfer() {
        let mut deps = mock_provenance_dependencies();
        setup_test_base(
            &mut deps.storage,
            &State {
                name: "contract_name".into(),
            },
        );

        let sender_address = Addr::unchecked("sender_address");
        let sender_info = mock_info(sender_address.as_str(), &[]);

        let reject_transfer_msg = ExecuteMsg::CancelTransfer {
            id: TRANSFER_ID.into(),
        };

        // execute cancel transfer
        let transfer_response = execute(
            deps.as_mut(),
            mock_env(),
            sender_info.clone(),
            reject_transfer_msg.clone(),
        );

        assert_load_transfer_error(transfer_response);
    }

    #[test]
    fn reject_transfer_success() {
        let mut deps = mock_provenance_dependencies();
        setup_test_base(
            &mut deps.storage,
            &State {
                name: "contract_name".into(),
            },
        );

        let sender_address = Addr::unchecked("sender_address");
        let transfer_address = Addr::unchecked("transfer_address");
        let recipient_address = Addr::unchecked("transfer_to");

        let test_marker: MarkerAccount =
            setup_restricted_marker_transfer(RESTRICTED_DENOM.into(), transfer_address.to_owned());
        mock_query_marker_response(&test_marker, &mut deps.querier);

        let amount = Uint128::new(3);
        let sender_info = mock_info(transfer_address.as_str(), &[]);

        store_test_transfer(
            &mut deps.storage,
            &Transfer {
                id: TRANSFER_ID.into(),
                sender: sender_address.to_owned(),
                denom: RESTRICTED_DENOM.into(),
                amount,
                recipient: recipient_address.to_owned(),
            },
        );

        let reject_transfer_msg = ExecuteMsg::RejectTransfer {
            id: TRANSFER_ID.into(),
        };

        // execute reject transfer
        let reject_response = execute(
            deps.as_mut(),
            mock_env(),
            sender_info.clone(),
            reject_transfer_msg.clone(),
        );

        let expected_coin = Coin {
            denom: RESTRICTED_DENOM.to_owned(),
            amount: amount.into(),
        };

        // verify approve transfer response
        match reject_response {
            Ok(response) => {
                assert_eq!(response.attributes.len(), 6);
                assert_eq!(
                    response.attributes[0],
                    attr("action", Action::Reject.to_string())
                );
                assert_eq!(response.attributes[1], attr("id", TRANSFER_ID));
                assert_eq!(response.attributes[2], attr("denom", RESTRICTED_DENOM));
                assert_eq!(response.attributes[3], attr("amount", amount.to_string()));
                assert_eq!(
                    response.attributes[4],
                    attr("sender", sender_address.to_owned())
                );
                assert_eq!(
                    response.attributes[5],
                    attr("admin", transfer_address.to_owned())
                );

                assert_eq!(response.messages.len(), 1);

                let expected_message: Binary = MsgTransferRequest {
                    amount: Some(expected_coin),
                    to_address: sender_address.to_string(),
                    from_address: MOCK_CONTRACT_ADDR.to_owned(),
                    administrator: MOCK_CONTRACT_ADDR.to_owned(),
                }
                .try_into()
                .unwrap();

                match &response.messages[0].msg {
                    CosmosMsg::Stargate { type_url, value } => {
                        assert_eq!(type_url, "/provenance.marker.v1.MsgTransferRequest");
                        assert_eq!(value, &expected_message);
                    }
                    _ => panic!("unexpected cosmos message"),
                }
            }
            Err(error) => {
                panic!("failed to reject transfer: {:?}", error)
            }
        }

        assert_eq!(
            None,
            TRANSFER_STORAGE
                .may_load(&deps.storage, TRANSFER_ID.as_bytes())
                .unwrap()
        );
    }

    #[test]
    fn reject_transfer_sent_funds_returns_error() {
        let mut deps = mock_provenance_dependencies();
        setup_test_base(
            &mut deps.storage,
            &State {
                name: "contract_name".into(),
            },
        );

        let sender_address = Addr::unchecked("sender_address");
        let transfer_address = Addr::unchecked("transfer_address");
        let recipient_address = Addr::unchecked("transfer_to");

        let test_marker: MarkerAccount =
            setup_restricted_marker_transfer(RESTRICTED_DENOM.into(), transfer_address.to_owned());
        mock_query_marker_response(&test_marker, &mut deps.querier);

        let amount = Uint128::new(3);
        let sender_info = mock_info(transfer_address.as_str(), &[coin(1, RESTRICTED_DENOM)]);

        let stored_transfer = Transfer {
            id: TRANSFER_ID.into(),
            sender: sender_address.to_owned(),
            denom: RESTRICTED_DENOM.into(),
            amount,
            recipient: recipient_address.to_owned(),
        };
        store_test_transfer(&mut deps.storage, &stored_transfer);

        let reject_transfer_msg = ExecuteMsg::RejectTransfer {
            id: TRANSFER_ID.into(),
        };

        // execute reject transfer
        let reject_response = execute(
            deps.as_mut(),
            mock_env(),
            sender_info.clone(),
            reject_transfer_msg.clone(),
        );

        assert_sent_funds_unsupported_error(reject_response);

        assert_eq!(
            stored_transfer,
            TRANSFER_STORAGE
                .load(&deps.storage, TRANSFER_ID.as_bytes())
                .unwrap()
        );
    }

    #[test]
    fn reject_transfer_unauthorized() {
        let mut deps = mock_provenance_dependencies();
        setup_test_base(
            &mut deps.storage,
            &State {
                name: "contract_name".into(),
            },
        );

        let transfer_address = Addr::unchecked("transfer_address");
        let sender_address = Addr::unchecked("sender_address");
        let recipient_address = Addr::unchecked("transfer_to");

        let test_marker =
            setup_restricted_marker_transfer(RESTRICTED_DENOM.into(), transfer_address.to_owned());
        mock_query_marker_response(&test_marker, &mut deps.querier);

        let amount = Uint128::new(3);
        let sender_info = mock_info(sender_address.as_str(), &[]);

        let stored_transfer = Transfer {
            id: TRANSFER_ID.into(),
            sender: sender_address.to_owned(),
            denom: RESTRICTED_DENOM.into(),
            amount,
            recipient: recipient_address.to_owned(),
        };
        store_test_transfer(&mut deps.storage, &stored_transfer);

        let reject_transfer_msg = ExecuteMsg::RejectTransfer {
            id: TRANSFER_ID.into(),
        };

        // execute reject transfer
        let transfer_response = execute(
            deps.as_mut(),
            mock_env(),
            sender_info.clone(),
            reject_transfer_msg.clone(),
        );

        // verify reject transfer response
        match transfer_response {
            Ok(..) => panic!("expected error, but ok"),
            Err(error) => match error {
                ContractError::Unauthorized { .. } => {}
                error => panic!("unexpected error: {:?}", error),
            },
        }

        assert_eq!(
            stored_transfer,
            TRANSFER_STORAGE
                .load(&deps.storage, TRANSFER_ID.as_bytes())
                .unwrap()
        );
    }

    #[test]
    fn reject_transfer_unknown_transfer() {
        let mut deps = mock_provenance_dependencies();
        setup_test_base(
            &mut deps.storage,
            &State {
                name: "contract_name".into(),
            },
        );

        let sender_address = Addr::unchecked("sender_address");
        let sender_info = mock_info(sender_address.as_str(), &[]);

        let reject_transfer_msg = ExecuteMsg::RejectTransfer {
            id: TRANSFER_ID.into(),
        };

        // execute reject transfer
        let transfer_response = execute(
            deps.as_mut(),
            mock_env(),
            sender_info.clone(),
            reject_transfer_msg.clone(),
        );

        assert_load_transfer_error(transfer_response);
    }

    #[test]
    fn query_transfer_by_id_test() {
        let mut deps = mock_provenance_dependencies();
        setup_test_base(
            &mut deps.storage,
            &State {
                name: "contract_name".into(),
            },
        );

        let sender_address = Addr::unchecked("sender_address");
        let recipient_address = Addr::unchecked("transfer_to");

        let amount = Uint128::new(3);

        let transfer = &Transfer {
            id: TRANSFER_ID.into(),
            sender: sender_address.to_owned(),
            denom: RESTRICTED_DENOM.into(),
            amount,
            recipient: recipient_address.to_owned(),
        };
        store_test_transfer(&mut deps.storage, transfer);

        let query_transfer_response = query(
            deps.as_ref(),
            mock_env(),
            QueryMsg::GetTransfer {
                id: TRANSFER_ID.into(),
            },
        );

        assert_eq!(to_binary(transfer), query_transfer_response);
    }

    #[test]
    fn query_contract_info() {
        let mut deps = mock_provenance_dependencies();
        setup_test_base(
            &mut deps.storage,
            &State {
                name: "contract_name".into(),
            },
        );

        let query_contract_info_response =
            query(deps.as_ref(), mock_env(), QueryMsg::GetContractInfo {});

        match query_contract_info_response {
            Ok(contract_info) => {
                assert_eq!(
                    contract_info,
                    to_binary(&CONFIG.load(&deps.storage).unwrap()).unwrap()
                )
            }
            Err(error) => panic!("unexpected error: {:?}", error),
        }
    }

    #[test]
    fn query_version_info() {
        let mut deps = mock_provenance_dependencies();
        setup_test_base(
            &mut deps.storage,
            &State {
                name: "contract_name".into(),
            },
        );

        let result = cw2::set_contract_version(deps.as_mut().storage, CRATE_NAME, PACKAGE_VERSION);
        match result {
            Ok(..) => {}
            Err(error) => panic!("unexpected error: {:?}", error),
        }

        let query_version_info_response =
            query(deps.as_ref(), mock_env(), QueryMsg::GetVersionInfo {});

        match query_version_info_response {
            Ok(version_info) => {
                assert_eq!(
                    version_info,
                    to_binary(&cw2::get_contract_version(&deps.storage).unwrap()).unwrap()
                )
            }
            Err(error) => panic!("unexpected error: {:?}", error),
        }
    }

    #[test]
    fn query_all_transfers() {
        let mut deps = mock_provenance_dependencies();
        setup_test_base(
            &mut deps.storage,
            &State {
                name: "contract_name".into(),
            },
        );

        let test_marker: MarkerAccount = setup_restricted_marker();
        mock_query_marker_response(&test_marker, &mut deps.querier);

        let amount = Uint128::new(1);
        let transfer_msg = ExecuteMsg::Transfer {
            id: TRANSFER_ID.into(),
            denom: RESTRICTED_DENOM.into(),
            amount: amount.into(),
            recipient: "transfer_to".into(),
        };

        let sender_info = mock_info("sender", &[]);

        let sender_balance = coin(1, RESTRICTED_DENOM);
        deps.querier
            .mock_querier
            .update_balance(Addr::unchecked("sender"), vec![sender_balance]);

        // execute create transfer
        execute(
            deps.as_mut(),
            mock_env(),
            sender_info.clone(),
            transfer_msg.clone(),
        )
        .unwrap();

        // verify transfer response
        let query_all_transfers_response =
            query(deps.as_ref(), mock_env(), QueryMsg::GetAllTransfers {}).unwrap();
        let all_transfers: Vec<Transfer> = from_binary(&query_all_transfers_response).unwrap();
        assert_eq!(1, all_transfers.len());
        assert_eq!(TRANSFER_ID.to_string(), all_transfers[0].id);
        assert_eq!(RESTRICTED_DENOM.to_string(), all_transfers[0].denom);
        assert_eq!(amount, all_transfers[0].amount);
        assert_eq!("transfer_to".to_string(), all_transfers[0].recipient);
    }

    #[test]
    fn query_all_transfers_empty() {
        let mut deps = mock_provenance_dependencies();
        setup_test_base(
            &mut deps.storage,
            &State {
                name: "contract_name".into(),
            },
        );

        let test_marker: MarkerAccount = setup_restricted_marker();
        mock_query_marker_response(&test_marker, &mut deps.querier);

        let sender_balance = coin(1, RESTRICTED_DENOM);
        deps.querier
            .mock_querier
            .update_balance(Addr::unchecked("sender"), vec![sender_balance]);

        // verify transfer response
        let query_all_transfers_response =
            query(deps.as_ref(), mock_env(), QueryMsg::GetAllTransfers {}).unwrap();
        let all_transfers: Vec<Transfer> = from_binary(&query_all_transfers_response).unwrap();
        assert_eq!(0, all_transfers.len());
    }

    fn assert_load_transfer_error(response: Result<Response, ContractError>) {
        match response {
            Ok(..) => panic!("expected error, but ok"),
            Err(error) => match error {
                ContractError::LoadTransferFailed { .. } => {}
                error => panic!("unexpected error: {:?}", error),
            },
        }
    }

    fn assert_sent_funds_unsupported_error(response: Result<Response, ContractError>) {
        match response {
            Ok(..) => panic!("expected error, but ok"),
            Err(error) => match error {
                ContractError::SentFundsUnsupported => {}
                error => panic!("unexpected error: {:?}", error),
            },
        }
    }

    fn setup_test_base(storage: &mut dyn Storage, contract_info: &State) {
        if let Err(error) = CONFIG.save(storage, &contract_info) {
            panic!("unexpected error: {:?}", error)
        }
    }

    fn store_test_transfer(storage: &mut dyn Storage, transfer: &Transfer) {
        if let Err(error) = TRANSFER_STORAGE.save(storage, transfer.id.as_bytes(), transfer) {
            panic!("unexpected error: {:?}", error)
        };
    }

    fn setup_restricted_marker() -> MarkerAccount {
        return MarkerAccount {
            base_account: Some(BaseAccount {
                address: "tp1l330sxue4suxz9dhc40e2pns0ymrytf8uz4squ".to_string(),
                pub_key: None,
                account_number: 10,
                sequence: 0,
            }),
            manager: "tp13pnzut8zdjaqht7aqe7kk4ww5zfq04jzlytnmu".to_string(),
            access_control: vec![AccessGrant {
                address: "tp13pnzut8zdjaqht7aqe7kk4ww5zfq04jzlytnmu".to_string(),
                permissions: vec![
                    Access::Burn.into(),
                    Access::Delete.into(),
                    Access::Deposit.into(),
                    Access::Transfer.into(),
                    Access::Mint.into(),
                    Access::Withdraw.into(),
                ],
            }],
            status: MarkerStatus::Active.into(),
            denom: "restricted_1".to_string(),
            supply: "1000".to_string(),
            marker_type: MarkerType::Restricted.into(),
            supply_fixed: false,
            allow_governance_control: true,
            allow_forced_transfer: false,
            required_attributes: vec![],
        };
    }

    fn setup_restricted_marker_transfer(denom: String, admin: Addr) -> MarkerAccount {
        return MarkerAccount {
            base_account: Some(BaseAccount {
                address: "tp1l330sxue4suxz9dhc40e2pns0ymrytf8uz4squ".to_string(),
                pub_key: None,
                account_number: 10,
                sequence: 0,
            }),
            manager: "".to_string(),
            access_control: vec![AccessGrant {
                address: admin.to_string(),
                permissions: vec![
                    Access::Burn.into(),
                    Access::Delete.into(),
                    Access::Deposit.into(),
                    Access::Transfer.into(),
                    Access::Mint.into(),
                    Access::Withdraw.into(),
                ],
            }],
            status: MarkerStatus::Active.into(),
            denom: denom,
            supply: "1000".to_string(),
            marker_type: MarkerType::Restricted.into(),
            supply_fixed: false,
            allow_governance_control: false,
            allow_forced_transfer: false,
            required_attributes: vec![],
        };
    }

    fn mock_query_marker_response(
        marker_account: &MarkerAccount,
        querier: &mut MockProvenanceQuerier,
    ) {
        let mock_marker_response = QueryMarkerResponse {
            marker: Some(Any {
                type_url: "/provenance.marker.v1.MarkerAccount".to_string(),
                value: marker_account.encode_to_vec(),
            }),
        };

        QueryMarkerRequest::mock_response(querier, mock_marker_response);
    }
}
