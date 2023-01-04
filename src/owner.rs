use std::fmt::Debug;

use cosmwasm_schema::cw_serde;
use cosmwasm_std::{
    Addr, Api, CustomQuery, DepsMut, MessageInfo, Response, StdError, StdResult, Storage,
};
use cw_storage_plus::Item;
use schemars::JsonSchema;
use thiserror::Error;

/// Returned from Owner.query()
#[cw_serde]
pub struct OwnerResponse {
    pub owner: Option<String>,
    pub proposed: Option<String>,
    pub initialized: bool,
    pub abolished: bool,
}

/// Errors returned from Owner state transitions
#[derive(Error, Debug, PartialEq)]
pub enum OwnerError {
    #[error("{0}")]
    Std(#[from] StdError),

    #[error("Caller is not owner")]
    NotOwner {},

    #[error("Caller is not the proposed owner")]
    NotProposedOwner {},

    #[error("Owner state transition was not valid")]
    StateTransitionError {},
}

type OwnerResult<T> = Result<T, OwnerError>;

/// The finite states that are possible
#[cw_serde]
enum OwnerState {
    A(OwnerUninitialized),
    B(OwnerSetNoneProposed),
    C(OwnerSetWithProposed),
    D(OwnerRoleAbolished),
}

#[cw_serde]
struct OwnerUninitialized;

impl OwnerUninitialized {
    pub fn initialize(&self, owner: &Addr) -> OwnerState {
        OwnerState::B(OwnerSetNoneProposed {
            owner: owner.clone(),
        })
    }

    pub fn abolish_owner_role(&self) -> OwnerState {
        OwnerState::D(OwnerRoleAbolished)
    }
}

#[cw_serde]
struct OwnerSetNoneProposed {
    owner: Addr,
}

impl OwnerSetNoneProposed {
    pub fn propose(self, proposed: &Addr) -> OwnerState {
        OwnerState::C(OwnerSetWithProposed {
            owner: self.owner,
            proposed: proposed.clone(),
        })
    }

    pub fn abolish_owner_role(self) -> OwnerState {
        OwnerState::D(OwnerRoleAbolished)
    }
}

#[cw_serde]
struct OwnerSetWithProposed {
    owner: Addr,
    proposed: Addr,
}

impl OwnerSetWithProposed {
    pub fn clear_proposed(self) -> OwnerState {
        OwnerState::B(OwnerSetNoneProposed { owner: self.owner })
    }

    pub fn accept_proposed(self) -> OwnerState {
        OwnerState::B(OwnerSetNoneProposed {
            owner: self.proposed,
        })
    }

    pub fn abolish_owner_role(self) -> OwnerState {
        OwnerState::D(OwnerRoleAbolished)
    }
}

#[cw_serde]
struct OwnerRoleAbolished;

#[cw_serde]
pub enum OwnerUpdate {
    /// Proposes a new owner to take role. Only current owner can execute.
    ProposeNewOwner { proposed: String },
    /// Clears the currently proposed owner. Only current owner can execute.
    ClearProposed,
    /// Promotes the proposed owner to be the current one. Only the proposed owner can execute.
    AcceptProposed,
    /// Throws away the keys to the Owner role forever. Once done, no owner can ever be set later.
    AbolishOwnerRole,
}

#[cw_serde]
pub enum OwnerInit {
    /// Sets the initial owner when none. No restrictions permissions to modify.
    SetInitialOwner { owner: String },
    /// Throws away the keys to the Owner role forever. Once done, no owner can ever be set later.
    AbolishOwnerRole,
}

/// A struct designed to help facilitate a two-step transition between contract owners safely.
/// It implements a finite state machine with dispatched events to manage state transitions.
/// State A: OwnerUninitialized
///     - No restrictions on who can initialize the owner role
/// State B: OwnerSetNoneProposed
///     - Once owner is set. Only they can execute the following updates:
///       - ProposeNewOwner
///       - ClearProposed
/// State C: OwnerSetWithProposed
///     - Only the proposed new owner can accept the new role via AcceptProposed {}
///     - The current owner can also clear the proposed new owner via ClearProposed {}
///
/// In every state, the owner (or on init, the initializer) can choose to abandon the role
/// and make the config immutable.
///
///```text
///                                                                  Clear Proposed
///                                                    +-------------------------------------^
///                                                    |                                     |
///                                                    v                                     |
/// +----------------+                      +----------------+                       +-------+--------+
/// | Owner: None    |   Initialize Owner   | Owner: Gabe    |   Propose New Owner   | Owner: Gabe    |
/// | Proposed: None +--------------------->| Proposed: None +---------------------->| Proposed: Joy  |
/// +-----+----------+                      ++---------------+                       +-------+----+---+
///       |                                  | Owner: Joy                                    |    |
///       |                                  | Proposed: None                                |    |
///   Abolish Role                           |      ^                                        |    |
///       |                *immutable        |      |              Accept Proposed           |    |
///       |            +----------------+    |      <----------------------------------------+    |
///       +----------->| Owner: None    |    |                                                    |
///                    | Proposed: None +<---+------------------ Abolish Role --------------------+
///                    +----------------+
/// ```
pub struct Owner<'a>(Item<'a, OwnerState>);

impl<'a> Owner<'a> {
    pub const fn new(namespace: &'a str) -> Self {
        Self(Item::new(namespace))
    }

    fn state(&self, storage: &'a dyn Storage) -> StdResult<OwnerState> {
        Ok(self
            .0
            .may_load(storage)?
            .unwrap_or(OwnerState::A(OwnerUninitialized)))
    }

    //--------------------------------------------------------------------------------------------------
    // Queries
    //--------------------------------------------------------------------------------------------------
    pub fn current(&self, storage: &'a dyn Storage) -> StdResult<Option<Addr>> {
        Ok(match self.state(storage)? {
            OwnerState::B(b) => Some(b.owner),
            OwnerState::C(c) => Some(c.owner),
            _ => None,
        })
    }

    pub fn is_owner(&self, storage: &'a dyn Storage, addr: &Addr) -> StdResult<bool> {
        match self.current(storage)? {
            Some(owner) if &owner == addr => Ok(true),
            _ => Ok(false),
        }
    }

    pub fn proposed(&self, storage: &'a dyn Storage) -> StdResult<Option<Addr>> {
        Ok(match self.state(storage)? {
            OwnerState::C(c) => Some(c.proposed),
            _ => None,
        })
    }

    pub fn is_proposed(&self, storage: &'a dyn Storage, addr: &Addr) -> StdResult<bool> {
        match self.proposed(storage)? {
            Some(proposed) if &proposed == addr => Ok(true),
            _ => Ok(false),
        }
    }

    pub fn query(&self, storage: &'a dyn Storage) -> StdResult<OwnerResponse> {
        Ok(OwnerResponse {
            owner: self.current(storage)?.map(Into::into),
            proposed: self.proposed(storage)?.map(Into::into),
            initialized: !matches!(self.state(storage)?, OwnerState::A(OwnerUninitialized)),
            abolished: matches!(self.state(storage)?, OwnerState::D(OwnerRoleAbolished)),
        })
    }

    //--------------------------------------------------------------------------------------------------
    // Mutations
    //--------------------------------------------------------------------------------------------------
    /// Execute inside instantiate fn
    pub fn initialize(
        &self,
        storage: &'a mut dyn Storage,
        api: &'a dyn Api,
        init_action: OwnerInit,
    ) -> OwnerResult<()> {
        let initial_state = self.state(storage)?;
        match initial_state {
            OwnerState::A(a) => {
                let new_state = match init_action {
                    OwnerInit::SetInitialOwner { owner } => {
                        let validated = api.addr_validate(&owner)?;
                        a.initialize(&validated)
                    }
                    OwnerInit::AbolishOwnerRole => a.abolish_owner_role(),
                };
                self.0.save(storage, &new_state)?;
                Ok(())
            }
            // Can only be in uninitialized state to call this fn
            _ => Err(OwnerError::StateTransitionError {}),
        }
    }

    /// Composes execute responses for owner state updates
    pub fn update<C, Q: CustomQuery>(
        &self,
        deps: DepsMut<Q>,
        info: MessageInfo,
        update: OwnerUpdate,
    ) -> OwnerResult<Response<C>>
    where
        C: Clone + Debug + PartialEq + JsonSchema,
    {
        let new_state = self.transition_state(deps.storage, deps.api, &info.sender, update)?;
        self.0.save(deps.storage, &new_state)?;

        let res = self.query(deps.storage)?;
        Ok(Response::new()
            .add_attribute("action", "update_owner")
            .add_attribute("owner", res.owner.unwrap_or_else(|| "None".to_string()))
            .add_attribute(
                "proposed",
                res.proposed.unwrap_or_else(|| "None".to_string()),
            )
            .add_attribute("sender", info.sender))
    }

    /// Executes owner state transitions
    fn transition_state(
        &self,
        storage: &'a mut dyn Storage,
        api: &'a dyn Api,
        sender: &Addr,
        event: OwnerUpdate,
    ) -> OwnerResult<OwnerState> {
        let state = self.state(storage)?;

        let new_state = match (state, event) {
            (OwnerState::B(b), OwnerUpdate::ProposeNewOwner { proposed }) => {
                let validated = api.addr_validate(&proposed)?;
                self.assert_owner(storage, sender)?;
                b.propose(&validated)
            }
            (OwnerState::B(b), OwnerUpdate::AbolishOwnerRole) => {
                self.assert_owner(storage, sender)?;
                b.abolish_owner_role()
            }
            (OwnerState::C(c), OwnerUpdate::AcceptProposed) => {
                self.assert_proposed(storage, sender)?;
                c.accept_proposed()
            }
            (OwnerState::C(c), OwnerUpdate::ClearProposed) => {
                self.assert_owner(storage, sender)?;
                c.clear_proposed()
            }
            (OwnerState::C(c), OwnerUpdate::AbolishOwnerRole) => {
                self.assert_owner(storage, sender)?;
                c.abolish_owner_role()
            }
            (_, _) => return Err(OwnerError::StateTransitionError {}),
        };
        Ok(new_state)
    }

    //--------------------------------------------------------------------------------------------------
    // Assertions
    //--------------------------------------------------------------------------------------------------
    /// Similar to is_owner() except it raises an exception if caller is not current owner
    pub fn assert_owner(&self, storage: &'a dyn Storage, caller: &Addr) -> OwnerResult<()> {
        if !self.is_owner(storage, caller)? {
            Err(OwnerError::NotOwner {})
        } else {
            Ok(())
        }
    }

    pub fn assert_proposed(&self, storage: &'a dyn Storage, caller: &Addr) -> OwnerResult<()> {
        if !self.is_proposed(storage, caller)? {
            Err(OwnerError::NotProposedOwner {})
        } else {
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {

    //--------------------------------------------------------------------------------------------------
    // Test invalid state transitions
    //--------------------------------------------------------------------------------------------------

    use crate::owner::OwnerState;
    use crate::OwnerUpdate::{AbolishOwnerRole, AcceptProposed, ClearProposed, ProposeNewOwner};
    use crate::{Owner, OwnerError, OwnerInit, OwnerResponse};
    use cosmwasm_std::testing::{mock_dependencies, mock_info};
    use cosmwasm_std::{Addr, Empty, Storage};

    #[test]
    fn invalid_uninitialized_state_transitions() {
        let mut deps = mock_dependencies();
        let sender = Addr::unchecked("peter_parker");
        let info = mock_info(sender.as_ref(), &[]);
        let owner = Owner::new("xyz");

        let err = owner
            .update::<Empty, Empty>(
                deps.as_mut(),
                info.clone(),
                ProposeNewOwner {
                    proposed: "abc".to_string(),
                },
            )
            .unwrap_err();
        assert_eq!(err, OwnerError::StateTransitionError {});

        let err = owner
            .update::<Empty, Empty>(deps.as_mut(), info.clone(), ClearProposed)
            .unwrap_err();
        assert_eq!(err, OwnerError::StateTransitionError {});

        let err = owner
            .update::<Empty, Empty>(deps.as_mut(), info.clone(), AcceptProposed)
            .unwrap_err();
        assert_eq!(err, OwnerError::StateTransitionError {});

        let err = owner
            .update::<Empty, Empty>(deps.as_mut(), info, AbolishOwnerRole)
            .unwrap_err();
        assert_eq!(err, OwnerError::StateTransitionError {});
    }

    #[test]
    fn invalid_owner_set_no_proposed_state_transitions() {
        let mut deps = mock_dependencies();
        let sender = Addr::unchecked("peter_parker");
        let info = mock_info(sender.as_ref(), &[]);
        let owner = Owner::new("xyz");

        let mut_deps = deps.as_mut();

        owner
            .initialize(
                mut_deps.storage,
                mut_deps.api,
                OwnerInit::SetInitialOwner {
                    owner: sender.to_string(),
                },
            )
            .unwrap();

        let err = owner
            .initialize(
                mut_deps.storage,
                mut_deps.api,
                OwnerInit::SetInitialOwner {
                    owner: "abc".to_string(),
                },
            )
            .unwrap_err();
        assert_eq!(err, OwnerError::StateTransitionError {});

        let err = owner
            .update::<Empty, Empty>(deps.as_mut(), info.clone(), ClearProposed)
            .unwrap_err();
        assert_eq!(err, OwnerError::StateTransitionError {});

        let err = owner
            .update::<Empty, Empty>(deps.as_mut(), info, AcceptProposed)
            .unwrap_err();
        assert_eq!(err, OwnerError::StateTransitionError {});
    }

    #[test]
    fn invalid_owner_set_with_proposed_state_transitions() {
        let mut deps = mock_dependencies();
        let sender = Addr::unchecked("peter_parker");
        let info = mock_info(sender.as_ref(), &[]);
        let owner = Owner::new("xyz");

        let mut_deps = deps.as_mut();

        owner
            .initialize(
                mut_deps.storage,
                mut_deps.api,
                OwnerInit::SetInitialOwner {
                    owner: sender.to_string(),
                },
            )
            .unwrap();

        owner
            .update::<Empty, Empty>(
                mut_deps,
                info.clone(),
                ProposeNewOwner {
                    proposed: "abc".to_string(),
                },
            )
            .unwrap();

        let mut_deps = deps.as_mut();

        let err = owner
            .initialize(
                mut_deps.storage,
                mut_deps.api,
                OwnerInit::SetInitialOwner {
                    owner: "abc".to_string(),
                },
            )
            .unwrap_err();
        assert_eq!(err, OwnerError::StateTransitionError {});

        let err = owner
            .update::<Empty, Empty>(
                deps.as_mut(),
                info,
                ProposeNewOwner {
                    proposed: "efg".to_string(),
                },
            )
            .unwrap_err();
        assert_eq!(err, OwnerError::StateTransitionError {});
    }

    #[test]
    fn invalid_owner_role_abolished_state_transitions() {
        let mut deps = mock_dependencies();
        let sender = Addr::unchecked("peter_parker");
        let info = mock_info(sender.as_ref(), &[]);
        let owner = Owner::new("xyz");

        let mut_deps = deps.as_mut();

        owner
            .initialize(mut_deps.storage, mut_deps.api, OwnerInit::AbolishOwnerRole)
            .unwrap();

        let err = owner
            .initialize(
                mut_deps.storage,
                mut_deps.api,
                OwnerInit::SetInitialOwner {
                    owner: "abc".to_string(),
                },
            )
            .unwrap_err();
        assert_eq!(err, OwnerError::StateTransitionError {});

        let err = owner
            .update::<Empty, Empty>(
                deps.as_mut(),
                info.clone(),
                ProposeNewOwner {
                    proposed: "efg".to_string(),
                },
            )
            .unwrap_err();
        assert_eq!(err, OwnerError::StateTransitionError {});

        let err = owner
            .update::<Empty, Empty>(deps.as_mut(), info.clone(), ClearProposed)
            .unwrap_err();
        assert_eq!(err, OwnerError::StateTransitionError {});

        let err = owner
            .update::<Empty, Empty>(deps.as_mut(), info.clone(), AcceptProposed)
            .unwrap_err();
        assert_eq!(err, OwnerError::StateTransitionError {});

        let err = owner
            .update::<Empty, Empty>(deps.as_mut(), info, AbolishOwnerRole)
            .unwrap_err();
        assert_eq!(err, OwnerError::StateTransitionError {});
    }

    //--------------------------------------------------------------------------------------------------
    // Test permissions
    //--------------------------------------------------------------------------------------------------

    #[test]
    fn initialize_owner_permissions() {
        let mut deps = mock_dependencies();
        let mut_deps = deps.as_mut();
        let owner = Owner::new("xyz");

        // Anyone can initialize
        owner
            .initialize(mut_deps.storage, mut_deps.api, OwnerInit::AbolishOwnerRole)
            .unwrap();

        let mut deps = mock_dependencies();
        let mut_deps = deps.as_mut();

        owner
            .initialize(
                mut_deps.storage,
                mut_deps.api,
                OwnerInit::SetInitialOwner {
                    owner: "xyz".to_string(),
                },
            )
            .unwrap();
    }

    #[test]
    fn propose_new_owner_permissions() {
        let mut deps = mock_dependencies();
        let sender = Addr::unchecked("peter_parker");
        let owner = Owner::new("xyz");

        let mut_deps = deps.as_mut();
        owner
            .initialize(
                mut_deps.storage,
                mut_deps.api,
                OwnerInit::SetInitialOwner {
                    owner: sender.to_string(),
                },
            )
            .unwrap();

        let bad_guy = Addr::unchecked("doc_oc");
        let info = mock_info(bad_guy.as_ref(), &[]);
        let err = owner
            .update::<Empty, Empty>(
                mut_deps,
                info,
                ProposeNewOwner {
                    proposed: bad_guy.to_string(),
                },
            )
            .unwrap_err();

        assert_eq!(err, OwnerError::NotOwner {})
    }

    #[test]
    fn clear_proposed_permissions() {
        let mut deps = mock_dependencies();
        let sender = Addr::unchecked("peter_parker");
        let info = mock_info(sender.as_ref(), &[]);
        let owner = Owner::new("xyz");

        let mut_deps = deps.as_mut();
        owner
            .initialize(
                mut_deps.storage,
                mut_deps.api,
                OwnerInit::SetInitialOwner {
                    owner: sender.to_string(),
                },
            )
            .unwrap();
        owner
            .update::<Empty, Empty>(
                mut_deps,
                info,
                ProposeNewOwner {
                    proposed: "miles_morales".to_string(),
                },
            )
            .unwrap();

        let bad_guy = Addr::unchecked("doc_oc");
        let info = mock_info(bad_guy.as_ref(), &[]);
        let err = owner
            .update::<Empty, Empty>(deps.as_mut(), info, ClearProposed)
            .unwrap_err();

        assert_eq!(err, OwnerError::NotOwner {})
    }

    #[test]
    fn accept_proposed_permissions() {
        let mut deps = mock_dependencies();
        let sender = Addr::unchecked("peter_parker");
        let info = mock_info(sender.as_ref(), &[]);
        let owner = Owner::new("xyz");

        let mut_deps = deps.as_mut();
        owner
            .initialize(
                mut_deps.storage,
                mut_deps.api,
                OwnerInit::SetInitialOwner {
                    owner: sender.to_string(),
                },
            )
            .unwrap();
        owner
            .update::<Empty, Empty>(
                mut_deps,
                info,
                ProposeNewOwner {
                    proposed: "miles_morales".to_string(),
                },
            )
            .unwrap();

        let bad_guy = Addr::unchecked("doc_oc");
        let info = mock_info(bad_guy.as_ref(), &[]);
        let err = owner
            .update::<Empty, Empty>(deps.as_mut(), info, AcceptProposed)
            .unwrap_err();

        assert_eq!(err, OwnerError::NotProposedOwner {})
    }

    #[test]
    fn abolish_owner_role_permissions() {
        let mut deps = mock_dependencies();
        let sender = Addr::unchecked("peter_parker");
        let owner = Owner::new("xyz");

        let mut_deps = deps.as_mut();
        owner
            .initialize(
                mut_deps.storage,
                mut_deps.api,
                OwnerInit::SetInitialOwner {
                    owner: sender.to_string(),
                },
            )
            .unwrap();

        let bad_guy = Addr::unchecked("doc_oc");
        let info = mock_info(bad_guy.as_ref(), &[]);
        let err = owner
            .update::<Empty, Empty>(deps.as_mut(), info, AbolishOwnerRole)
            .unwrap_err();

        assert_eq!(err, OwnerError::NotOwner {})
    }

    //--------------------------------------------------------------------------------------------------
    // Test success cases
    //--------------------------------------------------------------------------------------------------

    fn assert_uninitialized(storage: &dyn Storage, owner: &Owner) {
        let state = owner.state(storage).unwrap();
        match state {
            OwnerState::A(_) => {}
            _ => panic!("Should be in the OwnerUninitialized state"),
        }

        let current = owner.current(storage).unwrap();
        assert_eq!(current, None);

        let proposed = owner.proposed(storage).unwrap();
        assert_eq!(proposed, None);

        let res = owner.query(storage).unwrap();
        assert_eq!(
            res,
            OwnerResponse {
                owner: None,
                proposed: None,
                initialized: false,
                abolished: false,
            }
        );
    }

    #[test]
    fn uninitialized_state() {
        let deps = mock_dependencies();
        let owner = Owner::new("xyz");
        assert_uninitialized(deps.as_ref().storage, &owner);
    }

    #[test]
    fn initialize_owner() {
        let mut deps = mock_dependencies();
        let original_owner = Addr::unchecked("peter_parker");
        let owner = Owner::new("xyz");

        let mut_deps = deps.as_mut();
        owner
            .initialize(
                mut_deps.storage,
                mut_deps.api,
                OwnerInit::SetInitialOwner {
                    owner: original_owner.to_string(),
                },
            )
            .unwrap();

        let state = owner.state(mut_deps.storage).unwrap();
        match state {
            OwnerState::B(_) => {}
            _ => panic!("Should be in the OwnerSetNoneProposed state"),
        }

        let current = owner.current(mut_deps.storage).unwrap();
        assert_eq!(current, Some(original_owner.clone()));
        assert!(owner.is_owner(mut_deps.storage, &original_owner).unwrap());

        let proposed = owner.proposed(mut_deps.storage).unwrap();
        assert_eq!(proposed, None);

        let res = owner.query(mut_deps.storage).unwrap();
        assert_eq!(
            res,
            OwnerResponse {
                owner: Some(original_owner.to_string()),
                proposed: None,
                initialized: true,
                abolished: false,
            }
        );
    }

    #[test]
    fn propose_new_owner() {
        let mut deps = mock_dependencies();
        let original_owner = Addr::unchecked("peter_parker");
        let proposed_owner = Addr::unchecked("miles_morales");
        let info = mock_info(original_owner.as_ref(), &[]);
        let owner = Owner::new("xyz");

        let mut_deps = deps.as_mut();
        owner
            .initialize(
                mut_deps.storage,
                mut_deps.api,
                OwnerInit::SetInitialOwner {
                    owner: original_owner.to_string(),
                },
            )
            .unwrap();

        owner
            .update::<Empty, Empty>(
                mut_deps,
                info,
                ProposeNewOwner {
                    proposed: "miles_morales".to_string(),
                },
            )
            .unwrap();

        let storage = deps.as_mut().storage;

        let state = owner.state(storage).unwrap();
        match state {
            OwnerState::C(_) => {}
            _ => panic!("Should be in the OwnerSetWithProposed state"),
        }

        let current = owner.current(storage).unwrap();
        assert_eq!(current, Some(original_owner.clone()));
        assert!(owner.is_owner(storage, &original_owner).unwrap());

        let proposed = owner.proposed(storage).unwrap();
        assert_eq!(proposed, Some(proposed_owner.clone()));
        assert!(owner.is_proposed(storage, &proposed_owner).unwrap());

        let res = owner.query(storage).unwrap();
        assert_eq!(
            res,
            OwnerResponse {
                owner: Some(original_owner.to_string()),
                proposed: Some(proposed_owner.to_string()),
                initialized: true,
                abolished: false,
            }
        );
    }

    #[test]
    fn clear_proposed() {
        let mut deps = mock_dependencies();
        let original_owner = Addr::unchecked("peter_parker");
        let proposed_owner = Addr::unchecked("miles_morales");
        let info = mock_info(original_owner.as_ref(), &[]);
        let owner = Owner::new("xyz");

        let mut_deps = deps.as_mut();
        owner
            .initialize(
                mut_deps.storage,
                mut_deps.api,
                OwnerInit::SetInitialOwner {
                    owner: original_owner.to_string(),
                },
            )
            .unwrap();

        let mut_deps = deps.as_mut();
        owner
            .update::<Empty, Empty>(
                mut_deps,
                info.clone(),
                ProposeNewOwner {
                    proposed: "miles_morales".to_string(),
                },
            )
            .unwrap();

        let mut_deps = deps.as_mut();
        owner
            .update::<Empty, Empty>(mut_deps, info, ClearProposed)
            .unwrap();

        let storage = deps.as_mut().storage;

        let state = owner.state(storage).unwrap();
        match state {
            OwnerState::B(_) => {}
            _ => panic!("Should be in the OwnerSetNoneProposed state"),
        }

        let current = owner.current(storage).unwrap();
        assert_eq!(current, Some(original_owner.clone()));
        assert!(owner.is_owner(storage, &original_owner).unwrap());

        let proposed = owner.proposed(storage).unwrap();
        assert_eq!(proposed, None);
        assert!(!owner.is_proposed(storage, &proposed_owner).unwrap());

        let res = owner.query(storage).unwrap();
        assert_eq!(
            res,
            OwnerResponse {
                owner: Some(original_owner.to_string()),
                proposed: None,
                initialized: true,
                abolished: false,
            }
        );
    }

    #[test]
    fn accept_proposed() {
        let mut deps = mock_dependencies();
        let original_owner = Addr::unchecked("peter_parker");
        let proposed_owner = Addr::unchecked("miles_morales");
        let info = mock_info(original_owner.as_ref(), &[]);
        let owner = Owner::new("xyz");

        let mut_deps = deps.as_mut();
        owner
            .initialize(
                mut_deps.storage,
                mut_deps.api,
                OwnerInit::SetInitialOwner {
                    owner: original_owner.to_string(),
                },
            )
            .unwrap();

        let mut_deps = deps.as_mut();
        owner
            .update::<Empty, Empty>(
                mut_deps,
                info,
                ProposeNewOwner {
                    proposed: "miles_morales".to_string(),
                },
            )
            .unwrap();

        let info = mock_info(proposed_owner.as_ref(), &[]);
        let mut_deps = deps.as_mut();
        owner
            .update::<Empty, Empty>(mut_deps, info, AcceptProposed)
            .unwrap();

        let storage = deps.as_mut().storage;

        let state = owner.state(storage).unwrap();
        match state {
            OwnerState::B(_) => {}
            _ => panic!("Should be in the OwnerSetNoneProposed state"),
        }

        let current = owner.current(storage).unwrap();
        assert_eq!(current, Some(proposed_owner.clone()));
        assert!(owner.is_owner(storage, &proposed_owner).unwrap());

        let proposed = owner.proposed(storage).unwrap();
        assert_eq!(proposed, None);
        assert!(!owner.is_proposed(storage, &proposed_owner).unwrap());

        let res = owner.query(storage).unwrap();
        assert_eq!(
            res,
            OwnerResponse {
                owner: Some(proposed_owner.to_string()),
                proposed: None,
                initialized: true,
                abolished: false,
            }
        );
    }

    #[test]
    fn abolish_owner_role() {
        let mut deps = mock_dependencies();
        let original_owner = Addr::unchecked("peter_parker");
        let info = mock_info(original_owner.as_ref(), &[]);
        let owner = Owner::new("xyz");

        let mut_deps = deps.as_mut();
        owner
            .initialize(
                mut_deps.storage,
                mut_deps.api,
                OwnerInit::SetInitialOwner {
                    owner: original_owner.to_string(),
                },
            )
            .unwrap();

        let mut_deps = deps.as_mut();
        owner
            .update::<Empty, Empty>(mut_deps, info, AbolishOwnerRole)
            .unwrap();

        let storage = deps.as_mut().storage;

        let state = owner.state(storage).unwrap();
        match state {
            OwnerState::D(_) => {}
            _ => panic!("Should be in the OwnerRoleAbolished state"),
        }

        let current = owner.current(storage).unwrap();
        assert_eq!(current, None);
        assert!(!owner.is_owner(storage, &original_owner).unwrap());

        let proposed = owner.proposed(storage).unwrap();
        assert_eq!(proposed, None);
        assert!(!owner.is_proposed(storage, &original_owner).unwrap());

        let res = owner.query(storage).unwrap();
        assert_eq!(
            res,
            OwnerResponse {
                owner: None,
                proposed: None,
                initialized: true,
                abolished: true,
            }
        );
    }
}
