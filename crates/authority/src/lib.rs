// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! Who may do what, decided the same way every time.
//! Access control. `docs/concepts/risk_and_limits.md` carries why the table is exhaustive rather than a hierarchy.

/// What a person holds. Not ordered: a hierarchy would grant every new action to senior roles unasked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    /// The floor. Reads, and may stop the platform.
    Observer,
    /// Runs the platform day to day.
    Operator,
    /// Owns what the platform is allowed to risk.
    RiskOfficer,
}

impl Role {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Role::Observer => "observer",
            Role::Operator => "operator",
            Role::RiskOfficer => "risk-officer",
        }
    }

    /// The role written under this name, if this build knows one.
    /// Not `from_str`, so `"administrator".parse()` cannot be made to mean something.
    #[must_use]
    pub fn named(text: &str) -> Option<Self> {
        match text {
            "observer" => Some(Role::Observer),
            "operator" => Some(Role::Operator),
            "risk-officer" => Some(Role::RiskOfficer),
            _ => None,
        }
    }
}

/// Something a person asks the platform to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Halt,
    Reduce,
    /// Return to trading. The only one of these that relaxes anything.
    Resume,
    ChangeLimit,
    /// Mark an alarm as seen. It does not make the condition go away.
    AcknowledgeAlarm,
    ApproveRecommendation,
    /// Stop routing to one venue. What is held there becomes unknown, not nothing.
    WithdrawVenue,
    AdmitVenue,
    /// Accept into the record a position change that no fill explains (a transfer, a position
    /// held before the platform). It widens what the record holds, so it sits with limits.
    AdjustPosition,
    /// Cancel every order working when the command is carried out, protective reduce-only
    /// orders included. It changes no trading state: a halt is the way to stop new orders.
    CancelOrders,
    /// Close every position the record holds with a reduce-only market order, past the risk
    /// figures, and leave the node reducing (decision 0010).
    Flatten,
}

impl Action {
    /// Every action this build knows, and the only list of them: readers resolve names against
    /// it, so an action added here is readable everywhere it is written.
    pub const ALL: [Action; 11] = [
        Action::Halt,
        Action::Reduce,
        Action::Resume,
        Action::ChangeLimit,
        Action::AcknowledgeAlarm,
        Action::ApproveRecommendation,
        Action::WithdrawVenue,
        Action::AdmitVenue,
        Action::AdjustPosition,
        Action::CancelOrders,
        Action::Flatten,
    ];

    /// The action written under this name, if this build knows one.
    #[must_use]
    pub fn named(text: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|a| a.as_str() == text)
    }

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Action::Halt => "halt",
            Action::Reduce => "reduce",
            Action::Resume => "resume",
            Action::ChangeLimit => "change-limit",
            Action::AcknowledgeAlarm => "acknowledge-alarm",
            Action::ApproveRecommendation => "approve-recommendation",
            Action::WithdrawVenue => "withdraw-venue",
            Action::AdmitVenue => "admit-venue",
            Action::AdjustPosition => "adjust-position",
            Action::CancelOrders => "cancel-orders",
            Action::Flatten => "flatten",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    Permitted,
    Refused { role: Role, action: Action },
}

impl Decision {
    #[must_use]
    pub const fn is_permitted(self) -> bool {
        matches!(self, Decision::Permitted)
    }
}

impl core::fmt::Display for Decision {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Decision::Permitted => write!(f, "permitted"),
            // No article: a refusal an operator reads should not spend a word on grammar.
            Decision::Refused { role, action } => {
                write!(f, "{} may not {}", role.as_str(), action.as_str())
            }
        }
    }
}

/// Decide, exhaustively.
/// Every pair is written out, so a new `Action` or `Role` does not compile until decided. Stopping is open to every role.
#[must_use]
pub const fn decide(role: Role, action: Action) -> Decision {
    let permitted = match (role, action) {
        // Withdrawing a venue only narrows what the platform may do, like halting.
        (_, Action::Halt | Action::Reduce | Action::AcknowledgeAlarm | Action::WithdrawVenue) => {
            true
        }

        // Admitting one relaxes, so it sits with resuming rather than with withdrawing.
        (Role::Observer, Action::AdmitVenue) => false,
        (Role::Operator | Role::RiskOfficer, Action::AdmitVenue) => true,

        (Role::Observer, Action::Resume) => false,
        (Role::Operator | Role::RiskOfficer, Action::Resume) => true,

        // One person must not hold both halves: resuming and widening the limit.
        (Role::Observer | Role::Operator, Action::ChangeLimit) => false,
        (Role::RiskOfficer, Action::ChangeLimit) => true,

        // The operator resumes. Accepting what the record holds is the other half.
        (Role::Observer | Role::Operator, Action::AdjustPosition) => false,
        (Role::RiskOfficer, Action::AdjustPosition) => true,

        // It also removes protective orders, so it is not open to every role the way halting is.
        (Role::Observer, Action::CancelOrders) => false,
        (Role::Operator | Role::RiskOfficer, Action::CancelOrders) => true,

        // It trades at market and realises what the positions carry, so it sits with cancelling.
        (Role::Observer, Action::Flatten) => false,
        (Role::Operator | Role::RiskOfficer, Action::Flatten) => true,

        (Role::Observer, Action::ApproveRecommendation) => false,
        (Role::Operator | Role::RiskOfficer, Action::ApproveRecommendation) => true,
    };

    if permitted { Decision::Permitted } else { Decision::Refused { role, action } }
}

#[cfg(test)]
#[path = "../tests/unit/tests.rs"]
mod tests;
