use serde::{Deserialize, Serialize};

/// User information (simplified)
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct User {
    pub account_channel: String,
    pub aaid: String,
    pub base_currency: String,
}

impl Default for User {
    fn default() -> Self {
        Self {
            account_channel: String::new(),
            aaid: String::new(),
            base_currency: "HKD".to_string(),
        }
    }
}

impl User {
    pub fn get_account_channel(&self) -> &str {
        &self.account_channel
    }
}

/// Organization information
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct OrgInfo {
    pub name: String,
}

/// Account information
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Account {
    pub account_channel: String,
    pub aaid: String,
    pub account_name: String,
    pub account_type: String,
    pub org: OrgInfo,
}

impl Default for Account {
    fn default() -> Self {
        Self {
            account_channel: String::new(),
            aaid: String::new(),
            account_name: "Default Account".to_string(),
            account_type: "CashAccount".to_string(),
            org: OrgInfo {
                name: "Longbridge".to_string(),
            },
        }
    }
}

impl Account {
    pub fn is_open(&self) -> bool {
        self.account_type == "MarginAccount" || self.account_type == "CashAccount"
    }
}

/// Account list response
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AccountList {
    pub status: Vec<Account>,
}
