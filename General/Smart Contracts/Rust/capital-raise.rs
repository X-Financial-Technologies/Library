use cosmwasm_std::to_binary;
use cosmwasm_std::WasmMsg;
use cosmwasm_std::{
    coins, entry_point, Addr, Attribute, BankMsg, DepsMut, Env, Event, MessageInfo, Reply,
    Response, SubMsgResult,
};
use provwasm_std::{transfer_marker_coins, ProvenanceMsg};
use provwasm_std::{ProvenanceQuerier, ProvenanceQuery};
use serde::Serialize;
use std::vec::IntoIter;

use crate::error::contract_error;
use crate::error::ContractError;
use crate::exchange_asset::try_cancel_asset_exchanges;
use crate::exchange_asset::try_complete_asset_exchange;
use crate::exchange_asset::try_issue_asset_exchanges;
use crate::msg::{HandleMsg, SubscriptionMigrateMsg};
use crate::state::config;
use crate::state::eligible_subscriptions;
use crate::state::pending_subscriptions;
use crate::subscribe::try_accept_subscriptions;
use crate::subscribe::try_close_subscriptions;
use crate::subscribe::try_propose_subscription;
use crate::subscribe::try_upgrade_eligible_subscriptions;

pub type ContractResponse = Result<Response<ProvenanceMsg>, ContractError>;

#[entry_point]
pub fn reply(deps: DepsMut<ProvenanceQuery>, _env: Env, msg: Reply) -> ContractResponse {
    // look for a contract address from instantiating subscription contract
    if let SubMsgResult::Ok(response) = msg.result {
        if let Some(contract_address) = contract_address(&response.events) {
            let eligible = msg.id == 1;
            let mut storage = if eligible {
                eligible_subscriptions(deps.storage)
            } else {
                pending_subscriptions(deps.storage)
            };
            let mut subscriptions = storage.may_load()?.unwrap_or_default();
            subscriptions.insert(contract_address);
            storage.save(&subscriptions)?;
        } else {
            return contract_error("no contract address found");
        }
    } else {
        return contract_error("subscription contract instantiation failed");
    }

    Ok(Response::default())
}

fn contract_address(events: &[Event]) -> Option<Addr> {
    events.first().and_then(|event| {
        event
            .attributes
            .iter()
            .find(|attr| attr.key == "_contract_address")
            .map(|attr| Addr::unchecked(attr.value.clone()))
    })
}

#[derive(Serialize)]
struct EmptyArgs {}

#[entry_point]
pub fn execute(
    deps: DepsMut<ProvenanceQuery>,
    env: Env,
    info: MessageInfo,
    msg: HandleMsg,
) -> ContractResponse {
    match msg {
        HandleMsg::Recover { gp } => {
            let mut state = config(deps.storage).load()?;

            if info.sender != state.recovery_admin {
                return contract_error("only admin can recover raise");
            }

            state.gp = gp;
            config(deps.storage).save(&state)?;

            Ok(Response::default())
        }
        HandleMsg::UpdateRequiredAttestations {
            required_attestations,
        } => {
            let mut state = config(deps.storage).load()?;

            if info.sender != state.gp {
                return contract_error("only gp can update required attestations");
            }

            state.required_attestations = required_attestations;

            config(deps.storage).save(&state)?;

            Ok(Response::default())
        }
        HandleMsg::MigrateSubscriptions { subscriptions } => {
            let state = config(deps.storage).load()?;
            let migration_msg = SubscriptionMigrateMsg {
                required_capital_attribute: state.required_capital_attribute.clone(),
                capital_denom: Some(state.capital_denom.clone()),
            };
            Ok(
                Response::new().add_messages(subscriptions.iter().map(|sub| WasmMsg::Migrate {
                    contract_addr: sub.to_string(),
                    new_code_id: state.subscription_code_id,
                    msg: to_binary(&migration_msg).unwrap(),
                })),
            )
        }
        HandleMsg::ProposeSubscription { initial_commitment } => {
            try_propose_subscription(deps, env, info, initial_commitment)
        }
        HandleMsg::CloseSubscriptions { subscriptions } => {
            try_close_subscriptions(deps, info, subscriptions)
        }
        HandleMsg::UpdateEligibleSubscriptions { subscriptions } => {
            try_upgrade_eligible_subscriptions(deps, info, subscriptions)
        }
        HandleMsg::AcceptSubscriptions { subscriptions } => {
            try_accept_subscriptions(deps, info, subscriptions)
        }
        HandleMsg::IssueAssetExchanges { asset_exchanges } => {
            try_issue_asset_exchanges(deps, info, asset_exchanges)
        }
        HandleMsg::CancelAssetExchanges { cancellations } => {
            try_cancel_asset_exchanges(deps, info, cancellations)
        }
        HandleMsg::CompleteAssetExchange {
            exchanges,
            to,
            memo,
        } => try_complete_asset_exchange(deps, env, info, exchanges, to, memo),
        HandleMsg::IssueWithdrawal { to, amount, memo } => {
            let state = config(deps.storage).load()?;

            if info.sender != state.gp {
                return contract_error("only gp can redeem capital");
            }

            let attributes = match memo {
                Some(memo) => {
                    vec![Attribute {
                        key: String::from("memo"),
                        value: memo,
                    }]
                }
                None => vec![],
            };

            let response = match state.required_capital_attribute {
                None => {
                    let bank_send = BankMsg::Send {
                        to_address: to.to_string(),
                        amount: coins(amount as u128, &state.capital_denom),
                    };
                    Response::new()
                        .add_message(bank_send)
                        .add_attributes(attributes)
                }
                Some(required_capital_attribute) => {
                    if !query_attributes(deps, &to)
                        .any(|attr| attr.name == required_capital_attribute)
                    {
                        return contract_error(
                            format!(
                                "{} does not have required attribute of {}",
                                &to, &required_capital_attribute
                            )
                            .as_str(),
                        );
                    }

                    let marker_transfer = transfer_marker_coins(
                        amount as u128,
                        &state.capital_denom,
                        to,
                        env.contract.address,
                    )?;
                    Response::new()
                        .add_message(marker_transfer)
                        .add_attributes(attributes)
                }
            };

            Ok(response)
        }
    }
}

fn query_attributes(
    deps: DepsMut<ProvenanceQuery>,
    address: &Addr,
) -> IntoIter<provwasm_std::Attribute> {
    ProvenanceQuerier::new(&deps.querier)
        .get_attributes(address.clone(), None as Option<String>)
        .unwrap()
        .attributes
        .into_iter()
}

#[cfg(test)]
pub mod tests {
    use cosmwasm_std::testing::{mock_env, mock_info, MockApi, MockStorage, MOCK_CONTRACT_ADDR};
    use cosmwasm_std::{coin, SubMsgResponse};
    use cosmwasm_std::{Addr, OwnedDeps};
    use provwasm_mocks::{mock_dependencies, ProvenanceMockQuerier};
    use provwasm_std::MarkerMsgParams;

    use crate::mock::send_args;
    use crate::mock::{load_markers, marker_transfer_msg, msg_at_index};
    use crate::state::config_read;
    use crate::state::eligible_subscriptions_read;
    use crate::state::pending_subscriptions_read;
    use crate::state::State;

    use super::*;

    impl State {
        pub fn test_capital_coin() -> State {
            State {
                subscription_code_id: 100,
                recovery_admin: Addr::unchecked("marketpalace"),
                gp: Addr::unchecked("gp"),
                required_attestations: vec![vec![String::from("506c")].into_iter().collect()],
                commitment_denom: String::from("commitment_coin"),
                investment_denom: String::from("investment_coin"),
                capital_denom: String::from("capital_coin"),
                capital_per_share: 100,
                required_capital_attribute: None,
            }
        }

        pub fn test_restricted_capital_coin() -> State {
            State {
                subscription_code_id: 100,
                recovery_admin: Addr::unchecked("marketpalace"),
                gp: Addr::unchecked("gp"),
                required_attestations: vec![vec![String::from("506c")].into_iter().collect()],
                commitment_denom: String::from("commitment_coin"),
                investment_denom: String::from("investment_coin"),
                capital_denom: String::from("restricted_capital_coin"),
                capital_per_share: 100,
                required_capital_attribute: Some(String::from("capital.test")),
            }
        }
    }

    pub fn default_deps(
        update_state: Option<fn(&mut State)>,
    ) -> OwnedDeps<MockStorage, MockApi, ProvenanceMockQuerier, ProvenanceQuery> {
        let mut deps = mock_dependencies(&[]);

        let mut state = State::test_default();
        if let Some(update) = update_state {
            update(&mut state);
        }
        config(&mut deps.storage).save(&state).unwrap();

        deps
    }

    pub fn capital_coin_deps(
        update_state: Option<fn(&mut State)>,
    ) -> OwnedDeps<MockStorage, MockApi, ProvenanceMockQuerier, ProvenanceQuery> {
        let mut deps = mock_dependencies(&[]);

        let mut state = State::test_capital_coin();
        if let Some(update) = update_state {
            update(&mut state);
        }
        config(&mut deps.storage).save(&state).unwrap();

        deps
    }

    pub fn restricted_capital_coin_deps(
        update_state: Option<fn(&mut State)>,
    ) -> OwnedDeps<MockStorage, MockApi, ProvenanceMockQuerier, ProvenanceQuery> {
        let mut deps = mock_dependencies(&[]);

        let mut state = State::test_restricted_capital_coin();
        if let Some(update) = update_state {
            update(&mut state);
        }
        config(&mut deps.storage).save(&state).unwrap();

        deps
    }

    #[test]
    fn reply_pending() {
        let mut deps = default_deps(None);

        reply(
            deps.as_mut(),
            mock_env(),
            Reply {
                id: 0,
                result: cosmwasm_std::SubMsgResult::Ok(SubMsgResponse {
                    events: vec![
                        Event::new("contract address").add_attribute("_contract_address", "sub_1")
                    ],
                    data: None,
                }),
            },
        )
        .unwrap();

        // verify pending sub saved
        assert_eq!(
            "sub_1",
            pending_subscriptions_read(&deps.storage)
                .load()
                .unwrap()
                .iter()
                .next()
                .unwrap()
                .as_str()
        );
    }

    #[test]
    fn reply_eligible() {
        let mut deps = default_deps(None);

        reply(
            deps.as_mut(),
            mock_env(),
            Reply {
                id: 1,
                result: cosmwasm_std::SubMsgResult::Ok(SubMsgResponse {
                    events: vec![
                        Event::new("contract address").add_attribute("_contract_address", "sub_1")
                    ],
                    data: None,
                }),
            },
        )
        .unwrap();

        // verify pending sub saved
        assert_eq!(
            "sub_1",
            eligible_subscriptions_read(&deps.storage)
                .load()
                .unwrap()
                .iter()
                .next()
                .unwrap()
                .as_str()
        );
    }

    #[test]
    fn recover() {
        let mut deps = default_deps(None);

        execute(
            deps.as_mut(),
            mock_env(),
            mock_info("marketpalace", &vec![]),
            HandleMsg::Recover {
                gp: Addr::unchecked("gp_2"),
            },
        )
        .unwrap();

        // verify that gp has been updated
        let state = config_read(&deps.storage).load().unwrap();
        assert_eq!("gp_2", state.gp);
    }

    #[test]
    fn update_required_attestations() {
        let mut deps = default_deps(None);

        execute(
            deps.as_mut(),
            mock_env(),
            mock_info("gp", &vec![]),
            HandleMsg::UpdateRequiredAttestations {
                required_attestations: vec![],
            },
        )
        .unwrap();

        // verify that gp has been updated
        let state = config_read(&deps.storage).load().unwrap();
        assert_eq!(0, state.required_attestations.len());
    }

    #[test]
    fn fail_bad_actor_recover() {
        let mut deps = default_deps(None);

        let res = execute(
            deps.as_mut(),
            mock_env(),
            mock_info("bad_actor", &vec![]),
            HandleMsg::Recover {
                gp: Addr::unchecked("bad_actor"),
            },
        );
        assert!(res.is_err());

        // verify that gp has NOT been updated
        let state = config_read(&deps.storage).load().unwrap();
        assert_eq!("gp", state.gp);
    }

    #[test]
    fn issue_withdrawal() {
        let mut deps = capital_coin_deps(None);
        load_markers(&mut deps.querier);

        let res = execute(
            deps.as_mut(),
            mock_env(),
            mock_info("gp", &[]),
            HandleMsg::IssueWithdrawal {
                to: Addr::unchecked("omni"),
                amount: 10_000,
                memo: None,
            },
        )
        .unwrap();

        // verify that send message is sent
        assert_eq!(1, res.messages.len());
        let (to_address, coins) = send_args(msg_at_index(&res, 0));
        assert_eq!("omni", to_address);
        assert_eq!(10_000, coins.first().unwrap().amount.u128());
    }

    #[test]
    fn issue_restricted_coin_withdrawal() {
        let mut deps = restricted_capital_coin_deps(None);
        deps.querier
            .with_attributes("omni", &[("capital.test", "", "")]);
        load_markers(&mut deps.querier);

        let res = execute(
            deps.as_mut(),
            mock_env(),
            mock_info("gp", &[]),
            HandleMsg::IssueWithdrawal {
                to: Addr::unchecked("omni"),
                amount: 10_000,
                memo: None,
            },
        )
        .unwrap();

        // verify that send message is sent
        assert_eq!(1, res.messages.len());
        assert_eq!(
            &MarkerMsgParams::TransferMarkerCoins {
                coin: coin(10_000, "restricted_capital_coin"),
                to: Addr::unchecked("omni"),
                from: Addr::unchecked(MOCK_CONTRACT_ADDR),
            },
            marker_transfer_msg(msg_at_index(&res, 0)),
        );
    }

    #[test]
    fn issue_withdrawal_bad_actor() {
        let mut deps = default_deps(None);

        let res = execute(
            deps.as_mut(),
            mock_env(),
            mock_info("bad_actor", &[]),
            HandleMsg::IssueWithdrawal {
                to: Addr::unchecked("omni"),
                amount: 10_000,
                memo: None,
            },
        );
        assert!(res.is_err());
    }
}
