use crate::contract_info::{get_contract_info, set_contract_info, ContractInfo};
use crate::error::ContractError;
use crate::msg::{Authorize, ExecuteMsg, InstantiateMsg, MigrateMsg, QueryMsg, Validate};
use crate::state::{
    find_pledge_ids_with_assets, get_asset_ids, get_asset_ids_by_filter, get_assets,
    get_paydown_ids, get_paydowns, get_pledge_ids, get_pledges, load_paydown, load_pledge,
    remove_assets, save_paydown, save_pledge, set_assets_state, Asset, AssetState, ContractParty,
    Facility, Paydown, PaydownKind, PaydownSaleInfo, PaydownState, Pledge, PledgeState,
};
use crate::utils::{vec_contains, vec_has_any};
use cosmwasm_std::{
    attr, coins, entry_point, to_binary, Addr, BankMsg, Binary, Deps, DepsMut, Env, MessageInfo,
    Response, StdResult, Storage,
};
use provwasm_std::{
    activate_marker, bind_name, cancel_marker, create_marker, destroy_marker, finalize_marker,
    grant_marker_access, transfer_marker_coins, withdraw_coins, AccessGrant, Marker, MarkerAccess,
    MarkerType, NameBinding, ProvenanceMsg, ProvenanceQuerier,
};
use rust_decimal::prelude::{FromStr, ToPrimitive};
use rust_decimal::Decimal;
use std::ops::{Div, Mul};

pub const CONTRACT_VERSION: &str = env!("CARGO_PKG_VERSION");

fn marker_has_grant(marker: Marker, grant: AccessGrant) -> bool {
    let access = marker
        .permissions
        .into_iter()
        .find(|p| p.address == grant.address);

    let mut has_grant = false;
    if let Some(perm) = access {
        has_grant = vec_contains(&perm.permissions, &grant.permissions);
    }

    has_grant
}

// check if all of the specified assets are in the inventory with the optionally specified state (None = any state).
fn assets_in_inventory(
    storage: &dyn Storage,
    state: Option<AssetState>,
    assets: &[String],
) -> bool {
    let inventory_assets = get_asset_ids(storage, state, None, None).unwrap();
    vec_contains(&inventory_assets, &assets)
}

// check if any of the specified assets are in the inventory with the optionally specified state (None = any state).
fn any_assets_in_inventory(
    storage: &dyn Storage,
    state: Option<AssetState>,
    assets: &[String],
) -> bool {
    let inventory_assets = get_asset_ids(storage, state, None, None).unwrap();
    vec_has_any(&inventory_assets, &assets)
}

// smart contract initialization entrypoint
#[entry_point]
pub fn instantiate(
    deps: DepsMut,
    env: Env,
    info: MessageInfo,
    msg: InstantiateMsg,
) -> Result<Response<ProvenanceMsg>, ContractError> {
    // validate the message
    msg.validate()?;

    // get the advance rate
    let advance_rate = Decimal::from_str(&msg.facility.advance_rate).map_err(|_| {
        ContractError::InvalidFields {
            fields: vec![String::from("facility.advance_rate")],
        }
    })?;

    // calculate the total supply and distribution of facility marker
    let facility_marker_supply: u128 = 10u128.pow(advance_rate.scale() + 2);
    let facility_marker_to_warehouse: u128 = advance_rate
        .div(Decimal::from(100))
        .mul(Decimal::from(facility_marker_supply))
        .to_u128()
        .unwrap();
    let facility_marker_to_originator: u128 = facility_marker_supply - facility_marker_to_warehouse;

    // save contract info
    let contract_info = ContractInfo::new(
        info.sender,
        msg.bind_name,
        msg.contract_name,
        CONTRACT_VERSION.into(),
        msg.facility.clone(),
    );
    set_contract_info(deps.storage, &contract_info)?;

    // messages to include in transaction
    let mut messages = Vec::new();

    // create name binding
    messages.push(bind_name(
        contract_info.bind_name,
        env.contract.address.clone(),
        NameBinding::Restricted,
    )?);

    // create facility marker
    messages.push(create_marker(
        facility_marker_supply,
        msg.facility.marker_denom.clone(),
        MarkerType::Restricted,
    )?);

    // set privileges on the facility marker
    messages.push(grant_marker_access(
        msg.facility.marker_denom.clone(),
        env.contract.address,
        vec![
            MarkerAccess::Admin,
            MarkerAccess::Delete,
            MarkerAccess::Deposit,
            MarkerAccess::Transfer,
            MarkerAccess::Withdraw,
        ],
    )?);

    // finalize the facility marker
    messages.push(finalize_marker(msg.facility.marker_denom.clone())?);

    // activate the facility marker
    messages.push(activate_marker(msg.facility.marker_denom.clone())?);

    // withdraw the facility marker to the warehouse address
    messages.push(withdraw_coins(
        msg.facility.marker_denom.clone(),
        facility_marker_to_warehouse,
        msg.facility.marker_denom.clone(),
        Addr::unchecked(msg.facility.warehouse),
    )?);

    // withdraw the facility marker to the originator address
    messages.push(withdraw_coins(
        msg.facility.marker_denom.clone(),
        facility_marker_to_originator,
        msg.facility.marker_denom.clone(),
        Addr::unchecked(msg.facility.originator),
    )?);

    // build response
    Ok(Response::new()
        .add_messages(messages)
        .add_attributes(vec![
            attr(
                "contract_info",
                format!("{:?}", get_contract_info(deps.storage)?),
            ),
            attr("action", "init"),
        ]))
}

// smart contract execute entrypoint
#[entry_point]
pub fn execute(
    deps: DepsMut,
    env: Env,
    info: MessageInfo,
    msg: ExecuteMsg,
) -> Result<Response<ProvenanceMsg>, ContractError> {
    // validate the message
    msg.validate()?;

    // authorize the sender
    let contract_info = get_contract_info(deps.storage)?;
    msg.authorize(contract_info.clone(), info.sender.clone())?;

    match msg {
        ExecuteMsg::ProposePledge {
            id,
            assets,
            total_advance,
            asset_marker_denom,
        } => propose_pledge(
            deps,
            env,
            info,
            contract_info,
            id,
            assets,
            total_advance,
            asset_marker_denom,
        ),
        ExecuteMsg::AcceptPledge { id } => accept_pledge(deps, env, info, contract_info, id),
        ExecuteMsg::CancelPledge { id } => cancel_pledge(deps, env, info, contract_info, id),
        ExecuteMsg::ExecutePledge { id } => execute_pledge(deps, env, info, contract_info, id),
        ExecuteMsg::ProposePaydown {
            id,
            assets,
            total_paydown,
        } => propose_paydown(deps, env, info, contract_info, id, assets, total_paydown),
        ExecuteMsg::ProposePaydownAndSell {
            id,
            assets,
            total_paydown,
            buyer,
            purchase_price,
        } => propose_paydown_and_sell(
            deps,
            env,
            info,
            contract_info,
            id,
            assets,
            total_paydown,
            buyer,
            purchase_price,
        ),
        ExecuteMsg::AcceptPaydown { id } => accept_paydown(deps, env, info, contract_info, id),
        ExecuteMsg::CancelPaydown { id } => cancel_paydown(deps, env, info, contract_info, id),
        ExecuteMsg::ExecutePaydown { id } => execute_paydown(deps, env, info, contract_info, id),
    }
}

#[allow(clippy::too_many_arguments)]
fn propose_pledge(
    deps: DepsMut,
    env: Env,
    _info: MessageInfo,
    contract_info: ContractInfo,
    id: String,
    assets: Vec<String>,
    total_advance: u64,
    asset_marker_denom: String,
) -> Result<Response<ProvenanceMsg>, ContractError> {
    // ensure that a pledge with the specified id doesn't already exist
    let pledge = load_pledge(deps.storage, id.as_bytes());
    if let Ok(v) = pledge {
        return Err(ContractError::PledgeAlreadyExists { id: v.id });
    }

    // ensure that the assets are not in the inventory
    if any_assets_in_inventory(deps.storage, None, &assets) {
        return Err(ContractError::AssetsAlreadyPledged {});
    }

    // ensure the contract has privs on the escrow marker
    let querier = ProvenanceQuerier::new(&deps.querier);
    let escrow_marker =
        querier.get_marker_by_address(contract_info.facility.escrow_marker.clone())?;
    if !marker_has_grant(
        escrow_marker,
        AccessGrant {
            address: env.contract.address.clone(),
            permissions: vec![MarkerAccess::Transfer, MarkerAccess::Withdraw],
        },
    ) {
        return Err(ContractError::MissingEscrowMarkerGrant {});
    }

    // create the pledge
    let pledge = Pledge {
        id,
        assets,
        total_advance,
        asset_marker_denom: asset_marker_denom.clone(),
        state: PledgeState::Proposed,
    };

    // save the pledge
    save_pledge(deps.storage, &pledge.id.as_bytes(), &pledge)?;

    // update the asset(s) state in the facility inventory
    set_assets_state(deps.storage, AssetState::PledgeProposed, &pledge.assets)?;

    // TODO: using metadata module, we need to lookup the assets by id and change the value owner

    // messages to include in transaction
    let messages = vec![
        // create asset pool marker
        create_marker(1, asset_marker_denom.clone(), MarkerType::Restricted)?,
        // set privileges on the asset pool marker
        grant_marker_access(
            asset_marker_denom.clone(),
            env.contract.address,
            vec![
                MarkerAccess::Admin,
                MarkerAccess::Burn,
                MarkerAccess::Delete,
                MarkerAccess::Deposit,
                MarkerAccess::Mint,
                MarkerAccess::Transfer,
                MarkerAccess::Withdraw,
            ],
        )?,
        // finalize the asset pool marker
        finalize_marker(asset_marker_denom.clone())?,
        // activate the asset pool marker
        activate_marker(asset_marker_denom.clone())?,
        // withdraw the asset pool marker to the originator address
        withdraw_coins(
            asset_marker_denom.clone(),
            1,
            asset_marker_denom,
            Addr::unchecked(contract_info.facility.originator),
        )?,
    ];

    Ok(Response::new()
        .add_messages(messages)
        .add_attribute("action", "propose_pledge")
        .set_data(to_binary(&pledge)?))
}

fn accept_pledge(
    deps: DepsMut,
    env: Env,
    info: MessageInfo,
    contract_info: ContractInfo,
    id: String,
) -> Result<Response<ProvenanceMsg>, ContractError> {
    // locate the pledge
    let mut pledge = load_pledge(deps.storage, id.as_bytes())?;

    // only pledges that are in the "PROPOSED" state can be accepted
    if pledge.state != PledgeState::Proposed {
        return Err(ContractError::StateError {
            error: "Unable to accept pledge: Pledge is not in the 'proposed' state.".into(),
        });
    }

    // ensure the contract has privs on the escrow marker
    let querier = ProvenanceQuerier::new(&deps.querier);
    let escrow_marker =
        querier.get_marker_by_address(contract_info.facility.escrow_marker.clone())?;
    if !marker_has_grant(
        escrow_marker.clone(),
        AccessGrant {
            address: env.contract.address,
            permissions: vec![MarkerAccess::Transfer, MarkerAccess::Withdraw],
        },
    ) {
        return Err(ContractError::MissingEscrowMarkerGrant {});
    }

    // make sure that the warehouse sent the appropriate stablecoin
    let advance_funds = info
        .funds
        .get(0)
        .ok_or(ContractError::MissingPledgeAdvanceFunds {})?;
    if (advance_funds.denom != contract_info.facility.stablecoin_denom)
        || (advance_funds.amount != pledge.total_advance.into())
    {
        return Err(ContractError::InsufficientPledgeAdvanceFunds {
            need: pledge.total_advance.to_u128().unwrap(),
            need_denom: contract_info.facility.stablecoin_denom,
            received: advance_funds.amount.u128(),
            received_denom: advance_funds.denom.clone(),
        });
    }

    // messages to include in transaction
    let messages = vec![
        // forward stablecoin to escrow marker account
        BankMsg::Send {
            to_address: escrow_marker.address.to_string(),
            amount: coins(
                pledge.total_advance.into(),
                contract_info.facility.stablecoin_denom,
            ),
        },
    ];

    // update the pledge
    pledge.state = PledgeState::Accepted;
    save_pledge(deps.storage, &pledge.id.as_bytes(), &pledge)?;

    Ok(Response::new()
        .add_messages(messages)
        .add_attribute("action", "accept_pledge")
        .set_data(to_binary(&pledge)?))
}

fn cancel_pledge(
    deps: DepsMut,
    env: Env,
    _info: MessageInfo,
    contract_info: ContractInfo,
    id: String,
) -> Result<Response<ProvenanceMsg>, ContractError> {
    // locate the pledge
    let mut pledge = load_pledge(deps.storage, id.as_bytes())?;

    // only pledges that are in the "PROPOSED" or "ACCEPTED" states can be cancelled
    let remove_assets_from_escrow = true;
    let mut remove_advance_from_escrow = false;
    match pledge.state {
        PledgeState::Proposed => {}
        PledgeState::Accepted => {
            remove_advance_from_escrow = true;
        }
        _ => {
            return Err(ContractError::StateError {
                error:
                    "Unable to cancel pledge: Pledge is not in the 'proposed' or 'accepted' state."
                        .into(),
            })
        }
    }

    // ensure the contract has privs on the escrow marker
    let querier = ProvenanceQuerier::new(&deps.querier);
    let escrow_marker =
        querier.get_marker_by_address(contract_info.facility.escrow_marker.clone())?;
    if !marker_has_grant(
        escrow_marker.clone(),
        AccessGrant {
            address: env.contract.address,
            permissions: vec![MarkerAccess::Transfer, MarkerAccess::Withdraw],
        },
    ) {
        return Err(ContractError::MissingEscrowMarkerGrant {});
    }

    // messages to include in transaction
    let mut messages = Vec::new();

    // remove the advance from escrow back to the warehouse account
    if remove_advance_from_escrow {
        // withdraw advance funds from the escrow marker account to the warehouse
        messages.push(withdraw_coins(
            escrow_marker.denom,
            pledge.total_advance.into(),
            contract_info.facility.stablecoin_denom.clone(),
            contract_info.facility.warehouse,
        )?);
    }

    // remove the assets (asset marker) from escrow
    if remove_assets_from_escrow {
        let asset_marker = querier.get_marker_by_denom(pledge.asset_marker_denom.clone())?;

        // transfer the asset marker back to the marker supply
        messages.push(transfer_marker_coins(
            1,
            pledge.asset_marker_denom.clone(),
            asset_marker.address,
            contract_info.facility.originator,
        )?);

        // cancel the asset marker
        messages.push(cancel_marker(pledge.asset_marker_denom.clone())?);

        // destroy the asset marker
        messages.push(destroy_marker(pledge.asset_marker_denom.clone())?);
    }

    // update the pledge
    pledge.state = PledgeState::Cancelled;
    save_pledge(deps.storage, &pledge.id.as_bytes(), &pledge)?;

    // remove the assets from the inventory
    remove_assets(deps.storage, &pledge.assets)?;

    Ok(Response::new()
        .add_messages(messages)
        .add_attribute("action", "cancel_pledge")
        .set_data(to_binary(&pledge)?))
}

fn execute_pledge(
    deps: DepsMut,
    env: Env,
    _info: MessageInfo,
    contract_info: ContractInfo,
    id: String,
) -> Result<Response<ProvenanceMsg>, ContractError> {
    // locate the pledge
    let mut pledge = load_pledge(deps.storage, id.as_bytes())?;

    // only pledges that are in the "ACCEPTED" state can be executed
    if pledge.state != PledgeState::Accepted {
        return Err(ContractError::StateError {
            error: "Unable to execute pledge: Pledge is not in the 'accepted' state.".into(),
        });
    }

    // ensure the contract has privs on the escrow marker
    let querier = ProvenanceQuerier::new(&deps.querier);
    let escrow_marker =
        querier.get_marker_by_address(contract_info.facility.escrow_marker.clone())?;
    if !marker_has_grant(
        escrow_marker.clone(),
        AccessGrant {
            address: env.contract.address,
            permissions: vec![MarkerAccess::Transfer, MarkerAccess::Withdraw],
        },
    ) {
        return Err(ContractError::MissingEscrowMarkerGrant {});
    }

    // messages to include in transaction
    let messages = vec![
        // withdraw advance funds from the escrow marker account to the originator
        withdraw_coins(
            escrow_marker.denom,
            pledge.total_advance.into(),
            contract_info.facility.stablecoin_denom.clone(),
            contract_info.facility.originator,
        )?,
    ];

    // update the pledge
    pledge.state = PledgeState::Executed;
    save_pledge(deps.storage, &pledge.id.as_bytes(), &pledge)?;

    // update the asset(s) state in the facility inventory
    set_assets_state(deps.storage, AssetState::Inventory, &pledge.assets)?;

    Ok(Response::new()
        .add_messages(messages)
        .add_attribute("action", "execute_pledge"))
}

fn propose_paydown(
    deps: DepsMut,
    env: Env,
    info: MessageInfo,
    contract_info: ContractInfo,
    id: String,
    assets: Vec<String>,
    total_paydown: u64,
) -> Result<Response<ProvenanceMsg>, ContractError> {
    // ensure that a paydown with the specified id doesn't already exist
    let paydown = load_paydown(deps.storage, id.as_bytes());
    if let Ok(v) = paydown {
        return Err(ContractError::PaydownAlreadyExists { id: v.id });
    }

    // ensure that the included assets are in the inventory
    if !assets_in_inventory(deps.storage, Some(AssetState::Inventory), &assets) {
        return Err(ContractError::AssetsNotInInventory {});
    }

    // ensure the contract has privs on the escrow marker
    let querier = ProvenanceQuerier::new(&deps.querier);
    let escrow_marker =
        querier.get_marker_by_address(contract_info.facility.escrow_marker.clone())?;
    if !marker_has_grant(
        escrow_marker.clone(),
        AccessGrant {
            address: env.contract.address,
            permissions: vec![MarkerAccess::Transfer, MarkerAccess::Withdraw],
        },
    ) {
        return Err(ContractError::MissingEscrowMarkerGrant {});
    }

    // create the paydown
    let paydown = Paydown {
        id,
        assets,
        total_paydown,
        kind: PaydownKind::PaydownOnly,
        state: PaydownState::Proposed,
        parties_accepted: vec![],
        sale_info: None,
    };

    // make sure that the originator sent the appropriate stablecoin
    let paydown_funds = info
        .funds
        .get(0)
        .ok_or(ContractError::MissingPaydownFunds {})?;
    if (paydown_funds.denom != contract_info.facility.stablecoin_denom)
        || (paydown_funds.amount != paydown.total_paydown.into())
    {
        return Err(ContractError::InsufficientPaydownFunds {
            need: paydown.total_paydown.to_u128().unwrap(),
            need_denom: contract_info.facility.stablecoin_denom,
            received: paydown_funds.amount.u128(),
            received_denom: paydown_funds.denom.clone(),
        });
    }

    // messages to include in transaction
    let messages = vec![
        // forward stablecoin to escrow marker account
        BankMsg::Send {
            to_address: escrow_marker.address.to_string(),
            amount: coins(
                paydown.total_paydown.into(),
                contract_info.facility.stablecoin_denom,
            ),
        },
    ];

    // save the paydown
    save_paydown(deps.storage, &paydown.id.as_bytes(), &paydown)?;

    // update the asset(s) state in the facility inventory
    set_assets_state(deps.storage, AssetState::PaydownProposed, &paydown.assets)?;

    // get the pledges affected by this paydown
    let affected_pledges = find_pledge_ids_with_assets(
        deps.storage,
        paydown.assets,
        Some(PledgeState::Executed),
        None,
        None,
    )?;

    // TODO: Anything else to do at this state? How do we handle the asset marker(s) (assets being payed down
    //       can come from multiple pledges). CoNfUsEd!

    Ok(Response::new()
        .add_messages(messages)
        .add_attributes(vec![
            attr("action", "propose_paydown"),
            attr("affected_pledges", affected_pledges.join(",")),
        ]))
}

#[allow(clippy::too_many_arguments)]
fn propose_paydown_and_sell(
    deps: DepsMut,
    env: Env,
    info: MessageInfo,
    contract_info: ContractInfo,
    id: String,
    assets: Vec<String>,
    total_paydown: u64,
    buyer: Addr,
    purchase_price: u64,
) -> Result<Response<ProvenanceMsg>, ContractError> {
    // ensure that a paydown with the specified id doesn't already exist
    let paydown = load_paydown(deps.storage, id.as_bytes());
    if let Ok(v) = paydown {
        return Err(ContractError::PaydownAlreadyExists { id: v.id });
    }

    // ensure that the included assets are in the inventory
    if !assets_in_inventory(deps.storage, Some(AssetState::Inventory), &assets) {
        return Err(ContractError::AssetsNotInInventory {});
    }

    // ensure the contract has privs on the escrow marker
    let querier = ProvenanceQuerier::new(&deps.querier);
    let escrow_marker =
        querier.get_marker_by_address(contract_info.facility.escrow_marker.clone())?;
    if !marker_has_grant(
        escrow_marker.clone(),
        AccessGrant {
            address: env.contract.address,
            permissions: vec![MarkerAccess::Transfer, MarkerAccess::Withdraw],
        },
    ) {
        return Err(ContractError::MissingEscrowMarkerGrant {});
    }

    // create the paydown
    let paydown = Paydown {
        id,
        assets,
        total_paydown,
        kind: PaydownKind::PaydownAndSell,
        state: PaydownState::Proposed,
        parties_accepted: vec![],
        sale_info: Some(PaydownSaleInfo {
            buyer,
            price: purchase_price,
        }),
    };

    // make sure that the originator sent the appropriate stablecoin
    let paydown_funds = info
        .funds
        .get(0)
        .ok_or(ContractError::MissingPaydownFunds {})?;
    if (paydown_funds.denom != contract_info.facility.stablecoin_denom)
        || (paydown_funds.amount != paydown.total_paydown.into())
    {
        return Err(ContractError::InsufficientPaydownFunds {
            need: paydown.total_paydown.to_u128().unwrap(),
            need_denom: contract_info.facility.stablecoin_denom,
            received: paydown_funds.amount.u128(),
            received_denom: paydown_funds.denom.clone(),
        });
    }

    // messages to include in transaction
    let messages = vec![
        // forward stablecoin to escrow marker account
        BankMsg::Send {
            to_address: escrow_marker.address.to_string(),
            amount: coins(
                paydown.total_paydown.into(),
                contract_info.facility.stablecoin_denom,
            ),
        },
    ];

    // save the paydown
    save_paydown(deps.storage, &paydown.id.as_bytes(), &paydown)?;

    // update the asset(s) state in the facility inventory
    set_assets_state(deps.storage, AssetState::PaydownProposed, &paydown.assets)?;

    // get the pledges affected by this paydown
    let affected_pledges = find_pledge_ids_with_assets(
        deps.storage,
        paydown.assets,
        Some(PledgeState::Executed),
        None,
        None,
    )?;

    // TODO: Anything else to do at this state? How do we handle the asset marker(s) (assets being payed down
    //       can come from multiple pledges). CoNfUsEd!

    Ok(Response::new()
        .add_messages(messages)
        .add_attributes(vec![
            attr("action", "propose_paydown_and_sell"),
            attr("affected_pledges", affected_pledges.join(",")),
        ]))
}

fn accept_paydown(
    deps: DepsMut,
    env: Env,
    info: MessageInfo,
    contract_info: ContractInfo,
    id: String,
) -> Result<Response<ProvenanceMsg>, ContractError> {
    // locate the paydown
    let mut paydown = load_paydown(deps.storage, id.as_bytes())?;

    // extract the sale info
    let sale_info = paydown.sale_info.as_ref();

    // ensure the sender has a right to accept this paydown proposal
    let mut accepting_party = ContractParty::Warehouse;
    match paydown.kind {
        PaydownKind::PaydownOnly => {
            // only the warehouse in this facility can accept this paydown
            if contract_info.facility.warehouse != info.sender {
                return Err(ContractError::Unauthorized {});
            }
        }

        PaydownKind::PaydownAndSell => {
            // only the warehouse in this facility or the buyer of the assets can accept this paydown
            if contract_info.facility.warehouse == info.sender {
                accepting_party = ContractParty::Warehouse;
            } else if sale_info.unwrap().buyer == info.sender {
                accepting_party = ContractParty::Buyer;
            } else {
                return Err(ContractError::Unauthorized {});
            }
        }
    }

    // ensure that the accepting party hasn't already accepted
    if paydown
        .parties_accepted
        .clone()
        .into_iter()
        .find(|x| x == &accepting_party)
        != None
    {
        return Err(ContractError::PaydownPartyAlreadyAccepted {
            party: accepting_party,
        });
    }

    // only paydowns that are in the "PROPOSED" state can be accepted
    if paydown.state != PaydownState::Proposed {
        return Err(ContractError::StateError {
            error: "Unable to accept paydown: Paydown is not in the 'proposed' state.".into(),
        });
    }

    // ensure the contract has privs on the escrow marker
    let querier = ProvenanceQuerier::new(&deps.querier);
    let escrow_marker =
        querier.get_marker_by_address(contract_info.facility.escrow_marker.clone())?;
    if !marker_has_grant(
        escrow_marker.clone(),
        AccessGrant {
            address: env.contract.address,
            permissions: vec![MarkerAccess::Transfer, MarkerAccess::Withdraw],
        },
    ) {
        return Err(ContractError::MissingEscrowMarkerGrant {});
    }

    let mut messages = vec![];

    if accepting_party == ContractParty::Buyer {
        // make sure that the buyer sent the appropriate stablecoin
        let paydown_funds = info
            .funds
            .get(0)
            .ok_or(ContractError::MissingPurchaseFunds {})?;
        if (paydown_funds.denom != contract_info.facility.stablecoin_denom)
            || (paydown_funds.amount != sale_info.unwrap().price.into())
        {
            return Err(ContractError::InsufficientPurchaseFunds {
                need: sale_info.unwrap().price.to_u128().unwrap(),
                need_denom: contract_info.facility.stablecoin_denom,
                received: paydown_funds.amount.u128(),
                received_denom: paydown_funds.denom.clone(),
            });
        }

        // forward stablecoin to escrow marker account
        messages.push(
            BankMsg::Send {
                to_address: escrow_marker.address.to_string(),
                amount: coins(
                    sale_info.unwrap().price.into(),
                    contract_info.facility.stablecoin_denom,
                ),
            },
        );
    }

    // update the paydown
    paydown.parties_accepted.push(accepting_party);
    match paydown.kind {
        PaydownKind::PaydownOnly => {
            // for regular paydowns, only the warehouse needs to accept
            if vec_contains(&paydown.parties_accepted, &[ContractParty::Warehouse]) {
                paydown.state = PaydownState::Accepted;
            }
        }

        PaydownKind::PaydownAndSell => {
            // for paydown+sell, both the warehouse and the buyer needs to accept
            if vec_contains(
                &paydown.parties_accepted,
                &[ContractParty::Warehouse, ContractParty::Buyer],
            ) {
                paydown.state = PaydownState::Accepted;
            }
        }
    }
    save_paydown(deps.storage, &paydown.id.as_bytes(), &paydown)?;

    Ok(Response::new()
        .add_messages(messages)
        .add_attribute("action", "accept_paydown")
        .set_data(to_binary(&paydown)?))
}

fn cancel_paydown(
    deps: DepsMut,
    env: Env,
    _info: MessageInfo,
    contract_info: ContractInfo,
    id: String,
) -> Result<Response<ProvenanceMsg>, ContractError> {
    // locate the paydown
    let mut paydown = load_paydown(deps.storage, id.as_bytes())?;

    // only paydowns that are in the "PROPOSED" or "ACCEPTED" states can be cancelled=
    match paydown.state {
        PaydownState::Proposed => {}
        PaydownState::Accepted => {}
        _ => return Err(ContractError::StateError {
            error:
                "Unable to cancel paydown: Paydown is not in the 'proposed' or 'accepted' state."
                    .into(),
        }),
    }

    // ensure the contract has privs on the escrow marker
    let querier = ProvenanceQuerier::new(&deps.querier);
    let escrow_marker =
        querier.get_marker_by_address(contract_info.facility.escrow_marker.clone())?;
    if !marker_has_grant(
        escrow_marker.clone(),
        AccessGrant {
            address: env.contract.address,
            permissions: vec![MarkerAccess::Transfer, MarkerAccess::Withdraw],
        },
    ) {
        return Err(ContractError::MissingEscrowMarkerGrant {});
    }

    // messages to include in transaction
    let mut messages = vec![
        // withdraw paydown funds from the escrow marker account to the originator
        withdraw_coins(
            escrow_marker.clone().denom,
            paydown.total_paydown.into(),
            contract_info.facility.stablecoin_denom.clone(),
            contract_info.facility.originator,
        )?,
    ];

    if paydown.kind == PaydownKind::PaydownAndSell
        && vec_contains(&paydown.parties_accepted, &[ContractParty::Buyer])
    {
        // extract the sale info
        let sale_info = paydown.sale_info.as_ref();

        // withdraw purchase funds from the escrow marker account to the buyer
        messages.push(withdraw_coins(
            escrow_marker.denom,
            sale_info.unwrap().price.into(),
            contract_info.facility.stablecoin_denom,
            sale_info.unwrap().clone().buyer,
        )?);
    }

    // TODO: Anything else to do at this state (undo proposal)?

    // update the paydown
    paydown.state = PaydownState::Cancelled;
    save_paydown(deps.storage, &paydown.id.as_bytes(), &paydown)?;

    // update the asset(s) state in the facility inventory
    set_assets_state(deps.storage, AssetState::Inventory, &paydown.assets)?;

    Ok(Response::new()
        .add_messages(messages)
        .add_attribute("action", "cancel_paydown")
        .set_data(to_binary(&paydown)?))
}

fn execute_paydown(
    deps: DepsMut,
    env: Env,
    _info: MessageInfo,
    contract_info: ContractInfo,
    id: String,
) -> Result<Response<ProvenanceMsg>, ContractError> {
    // locate the paydown
    let mut paydown = load_paydown(deps.storage, id.as_bytes())?;

    // only paydowns that are in the "ACCEPTED" state can be executed
    if paydown.state != PaydownState::Accepted {
        return Err(ContractError::StateError {
            error: "Unable to execute paydown: Paydown is not in the 'accepted' state.".into(),
        });
    }

    // ensure the contract has privs on the escrow marker
    let querier = ProvenanceQuerier::new(&deps.querier);
    let escrow_marker =
        querier.get_marker_by_address(contract_info.facility.escrow_marker.clone())?;
    if !marker_has_grant(
        escrow_marker.clone(),
        AccessGrant {
            address: env.contract.address,
            permissions: vec![MarkerAccess::Transfer, MarkerAccess::Withdraw],
        },
    ) {
        return Err(ContractError::MissingEscrowMarkerGrant {});
    }

    // messages to include in transaction
    let mut messages = vec![
        // withdraw advance funds from the escrow marker account to the warehouse
        withdraw_coins(
            escrow_marker.clone().denom,
            paydown.total_paydown.into(),
            contract_info.facility.stablecoin_denom.clone(),
            contract_info.facility.warehouse,
        )?,
    ];

    if paydown.kind == PaydownKind::PaydownAndSell {
        // withdraw purchase funds from the escrow marker account to the originator
        messages.push(withdraw_coins(
            escrow_marker.denom,
            paydown.sale_info.as_ref().unwrap().price.into(),
            contract_info.facility.stablecoin_denom.clone(),
            contract_info.facility.originator.clone(),
        )?);
    }

    // TODO: value ownership change for asset markers. Waiting on metadata module.

    // update the paydown
    paydown.state = PaydownState::Executed;
    save_paydown(deps.storage, &paydown.id.as_bytes(), &paydown)?;

    // remove the assets from the facility inventory
    remove_assets(deps.storage, &paydown.assets)?;

    // get the current inventory
    let inventory = list_inventory(deps.storage)?;

    // get the pledges affected by this paydown
    let affected_pledges = find_pledge_ids_with_assets(
        deps.storage,
        paydown.assets,
        Some(PledgeState::Executed),
        None,
        None,
    )?;

    // get the pledges that are closed by this paydown
    let closed_pledges: Vec<String> = affected_pledges
        .iter()
        .filter(|id| {
            !vec_has_any(
                &inventory,
                &load_pledge(deps.storage, id.as_bytes()).unwrap().assets,
            )
        })
        .map(String::from)
        .collect();

    // update the state on the closed pledges
    for pledge_id in &closed_pledges {
        // load the pledge
        let mut pledge = get_pledge(deps.storage, String::from(pledge_id))?;

        // get the asset marker for the pledge
        let asset_marker = querier.get_marker_by_denom(pledge.asset_marker_denom.clone())?;

        // update the pledge
        pledge.state = PledgeState::Closed;
        save_pledge(deps.storage, &pledge.id.as_bytes(), &pledge)?;

        // transfer the asset marker back to the marker supply
        messages.push(transfer_marker_coins(
            1,
            pledge.asset_marker_denom.clone(),
            asset_marker.address,
            contract_info.facility.originator.clone(),
        )?);

        // cancel the asset marker
        messages.push(cancel_marker(pledge.asset_marker_denom.clone())?);

        // destroy the asset marker
        messages.push(destroy_marker(pledge.asset_marker_denom.clone())?);
    }

    Ok(Response::new()
        .add_messages(messages)
        .add_attributes(vec![
            attr("action", "execute_paydown"),
            attr("affected_pledges", affected_pledges.join(",")),
            attr("closed_pledges", closed_pledges.join(",")),
        ]))
}

fn get_facility_info(store: &dyn Storage) -> StdResult<Facility> {
    let contract_info = get_contract_info(store)?;
    Ok(contract_info.facility)
}

fn get_pledge(store: &dyn Storage, id: String) -> StdResult<Pledge> {
    load_pledge(store, id.as_bytes())
}

fn list_pledge_ids(store: &dyn Storage) -> StdResult<Vec<String>> {
    get_pledge_ids(store, None, None, None)
}

fn list_pledges(store: &dyn Storage) -> StdResult<Vec<Pledge>> {
    get_pledges(store, None, None, None)
}

fn list_pledge_proposals(store: &dyn Storage) -> StdResult<Vec<Pledge>> {
    get_pledges(store, Some(PledgeState::Proposed), None, None)
}

fn list_paydown_ids(store: &dyn Storage) -> StdResult<Vec<String>> {
    get_paydown_ids(store, None, None, None)
}

fn list_paydowns(store: &dyn Storage) -> StdResult<Vec<Paydown>> {
    get_paydowns(store, None, None, None)
}

fn list_paydown_proposals(store: &dyn Storage) -> StdResult<Vec<Paydown>> {
    get_paydowns(store, Some(PaydownState::Proposed), None, None)
}

fn get_paydown(store: &dyn Storage, id: String) -> StdResult<Paydown> {
    load_paydown(store, id.as_bytes())
}

fn list_assets(store: &dyn Storage) -> StdResult<Vec<Asset>> {
    get_assets(store, None, None, None)
}

// Get a list of the assets ids in the inventory.
// NOTE: An asset proposed for paydown is still technically in the inventory, so we include
// them in the filter.
fn list_inventory(store: &dyn Storage) -> StdResult<Vec<String>> {
    get_asset_ids_by_filter(
        store,
        vec![AssetState::Inventory, AssetState::PaydownProposed],
        None,
        None,
    )
}

// smart contract query entrypoint
#[entry_point]
pub fn query(deps: Deps, _env: Env, msg: QueryMsg) -> StdResult<Binary> {
    match msg {
        QueryMsg::GetContractInfo {} => to_binary(&get_contract_info(deps.storage)?),
        QueryMsg::GetFacilityInfo {} => to_binary(&get_facility_info(deps.storage)?),
        QueryMsg::GetPaydown { id } => to_binary(&get_paydown(deps.storage, id)?),
        QueryMsg::GetPledge { id } => to_binary(&get_pledge(deps.storage, id)?),
        QueryMsg::ListAssets {} => to_binary(&list_assets(deps.storage)?),
        QueryMsg::ListInventory {} => to_binary(&list_inventory(deps.storage)?),
        QueryMsg::ListPledgeIds {} => to_binary(&list_pledge_ids(deps.storage)?),
        QueryMsg::ListPledgeProposals {} => to_binary(&list_pledge_proposals(deps.storage)?),
        QueryMsg::ListPledges {} => to_binary(&list_pledges(deps.storage)?),
        QueryMsg::ListPaydownIds {} => to_binary(&list_paydown_ids(deps.storage)?),
        QueryMsg::ListPaydownProposals {} => to_binary(&list_paydown_proposals(deps.storage)?),
        QueryMsg::ListPaydowns {} => to_binary(&list_paydowns(deps.storage)?),
    }
}

// smart contract migrate/upgrade entrypoint
#[entry_point]
pub fn migrate(deps: DepsMut, _env: Env, _msg: MigrateMsg) -> StdResult<Response> {
    // always update version info
    let mut contract_info = get_contract_info(deps.storage)?;
    contract_info.version = CONTRACT_VERSION.into();
    set_contract_info(deps.storage, &contract_info)?;

    Ok(Response::default())
}
