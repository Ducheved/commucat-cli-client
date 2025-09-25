use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Group {
    pub id: String,
    pub name: String,
    pub owner: String,
    pub members: HashMap<String, GroupRole>,
    pub created_at: i64,
    pub relay: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum GroupRole {
    Owner,
    Admin,
    Member,
}

#[derive(Debug)]
pub enum GroupAction {
    Invite,
    Kick,
    ChangeRole,
    SendMessage,
}

impl Group {
    pub fn new(id: String, name: String, owner: String) -> Self {
        let mut members = HashMap::new();
        members.insert(owner.clone(), GroupRole::Owner);
        Group {
            id,
            name,
            owner,
            members,
            created_at: Utc::now().timestamp(),
            relay: true,
        }
    }

    pub fn add_member(&mut self, device_id: String, role: GroupRole) -> bool {
        if self.members.contains_key(&device_id) {
            return false;
        }
        self.members.insert(device_id, role);
        true
    }

    pub fn remove_member(&mut self, device_id: &str) -> bool {
        if device_id == self.owner {
            return false;
        }
        self.members.remove(device_id).is_some()
    }

    pub fn change_role(&mut self, device_id: &str, new_role: GroupRole) -> bool {
        if device_id == self.owner && new_role != GroupRole::Owner {
            return false;
        }
        if let Some(role) = self.members.get_mut(device_id) {
            *role = new_role;
            true
        } else {
            false
        }
    }

    pub fn has_permission(&self, device_id: &str, action: GroupAction) -> bool {
        self.members.get(device_id).is_some_and(|role| match role {
            GroupRole::Owner => true,
            GroupRole::Admin => matches!(
                action,
                GroupAction::Invite
                    | GroupAction::Kick
                    | GroupAction::ChangeRole
                    | GroupAction::SendMessage
            ),
            GroupRole::Member => matches!(action, GroupAction::SendMessage),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn owner_permissions_cover_all_actions() {
        let group = Group::new(
            "group-1".to_string(),
            "Test".to_string(),
            "owner".to_string(),
        );
        assert!(group.has_permission("owner", GroupAction::Invite));
        assert!(group.has_permission("owner", GroupAction::Kick));
        assert!(group.has_permission("owner", GroupAction::ChangeRole));
        assert!(group.has_permission("owner", GroupAction::SendMessage));
    }

    #[test]
    fn admin_permissions_respected() {
        let mut group = Group::new(
            "group-2".to_string(),
            "Team".to_string(),
            "owner".to_string(),
        );
        group.add_member("admin".to_string(), GroupRole::Admin);
        assert!(group.has_permission("admin", GroupAction::Invite));
        assert!(group.has_permission("admin", GroupAction::Kick));
        assert!(group.change_role("admin", GroupRole::Member));
        assert!(group.remove_member("admin"));
    }
}
