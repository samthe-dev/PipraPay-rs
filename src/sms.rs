//! SMS sender whitelist and MFS message parsing.
//!
//! Faithful port of PHP `senderWhitelist()` and `MFSMessageVerified()`
//! (`pp-functions.php`). Patterns are tried in priority order (highest
//! first) per provider; only the patterns PHP has uncommented are active.
//! telecash and ipay have whitelist entries but no parse regex, so they
//! always fail to parse, exactly as in PHP.

use regex::Regex;

pub const SENDERS: &[&str] = &[
    "bkash", "nagad", "16216", "upay", "tap.", "ibbl .", "01847-348685", "16259", "pathaopay",
    "telecash", "09638-900800",
];

/// Provider info from `senderWhitelist()`: (provider_key, name, currency,
/// balance_verify). Sender match is exact on the lowercased sender.
pub fn sender_whitelist(
    sender: &str,
) -> Option<(&'static str, &'static str, &'static str, &'static str)> {
    let s = sender.trim().to_lowercase();
    for (key, name, currency, balance_verify, senders) in PROVIDERS {
        if senders.contains(&s.as_str()) {
            return Some((*key, *name, *currency, *balance_verify));
        }
    }
    None
}

const PROVIDERS: &[(&str, &str, &str, &str, &[&str])] = &[
    ("bkash", "bKash", "BDT", "true", &["bkash"]),
    ("nagad", "Nagad", "BDT", "true", &["nagad"]),
    ("rocket", "Rocket", "BDT", "true", &["16216"]),
    ("upay", "Upay", "BDT", "true", &["upay"]),
    ("tap", "Tap", "USD", "true", &["tap."]),
    ("cellfin", "Cellfin", "BDT", "false", &["ibbl ."]),
    ("okwallet", "Ok Wallet", "BDT", "true", &["01847-348685"]),
    ("mcash", "mCash", "BDT", "true", &["16259"]),
    ("pathaopay", "Pathao Pay", "BDT", "true", &["pathaopay"]),
    ("telecash", "TeleCash", "BDT", "true", &["telecash"]),
    ("ipay", "Ipay", "BDT", "true", &["09638-900800"]),
];

/// Parse a money string like "15,230.00" into minor units (x10^8).
/// Non-numeric input gives 0, like PHP `money_sanitize()` + decimal math.
pub fn parse_money(value: &str) -> i64 {
    let cleaned: String = value.chars().filter(|c| *c != ',').collect();
    let cleaned = cleaned.trim();
    if cleaned.is_empty() {
        return 0;
    }
    let (neg, digits) = match cleaned.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, cleaned),
    };
    let mut parts = digits.splitn(2, '.');
    let int_part: i64 = parts.next().unwrap_or("0").parse().unwrap_or(0);
    let frac = parts.next().unwrap_or("");
    let frac_padded = format!("{frac:0<8}");
    let frac_part: i64 = frac_padded[..frac_padded.len().min(8)].parse().unwrap_or(0);
    let total = int_part.saturating_mul(100_000_000).saturating_add(frac_part);
    if neg {
        -total
    } else {
        total
    }
}

/// Format minor units back to a plain decimal string with 2 places,
/// like PHP `intToMoney(..., 2)`.
pub fn money_to_string(minor: i64) -> String {
    let neg = minor < 0;
    let v = minor.unsigned_abs();
    format!(
        "{}{}.{:02}",
        if neg { "-" } else { "" },
        v / 100_000_000,
        (v % 100_000_000) / 1_000_000
    )
}

#[derive(Debug, Clone, PartialEq)]
pub struct ParsedSms {
    pub type_: String,
    /// Amount as the SMS reported it, commas stripped ("500.00").
    pub amount: String,
    pub amount_minor: i64,
    /// Balance as the SMS reported it, commas stripped; None when the
    /// pattern has no balance (cellfin).
    pub balance: Option<String>,
    pub balance_minor: Option<i64>,
    /// Payer number from the message.
    pub sender: String,
    pub trxid: String,
    pub datetime: Option<String>,
}

struct Format {
    type_: &'static str,
    priority: u8,
    pattern: &'static str,
    /// Capture-group names in order, matching the regex groups.
    map: &'static [&'static str],
}

fn formats(provider: &str) -> &'static [Format] {
    match provider {
        "bkash" => &[
            Format { type_: "Personal", priority: 100, pattern: r"You have received Tk ([\d,.]+) from (\d+)\.(?:\s*Ref[:\-]?\s*(\S+))? Fee Tk ([\d,.]+)\. Balance Tk ([\d,.]+)\. TrxID ([A-Z0-9]+) at ([\d\/:\s]+)", map: &["amount", "sender", "ref", "fee", "balance", "trxid", "datetime"] },
            Format { type_: "Personal", priority: 90, pattern: r"Cash In Tk ([\d,.]+) from (\d+) successful\. Fee Tk ([\d,.]+)\. Balance Tk ([\d,.]+)\. TrxID ([A-Z0-9]+) at ([\d\/:\s]+)", map: &["amount", "sender", "fee", "balance", "trxid", "datetime"] },
            Format { type_: "Merchant", priority: 80, pattern: r"You have received payment Tk ([\d,.]+) from ([0-9X]+)\.(?:\s*Ref[:\-]?\s*(\S+))? Fee Tk ([\d,.]+)\. Balance Tk ([\d,.]+)\. TrxID ([A-Z0-9]+) at ([\d\/:\s]+)", map: &["amount", "sender", "ref", "fee", "balance", "trxid", "datetime"] },
        ],
        "nagad" => &[
            Format { type_: "Personal", priority: 100, pattern: r"Money Received\. Amount: Tk ([\d,.]+) Sender: (\d+)(?:\s*Ref[:\-]?\s*(\S+))? TxnID: ([A-Z0-9]+) Balance: Tk ([\d,.]+) ([\d\/:\s]+)", map: &["amount", "sender", "ref", "trxid", "balance", "datetime"] },
            Format { type_: "Personal", priority: 90, pattern: r"Cash In Received\. Amount: Tk ([\d,.]+) Uddokta: (\d+) TxnID: ([A-Z0-9]+) Balance: ([\d,.]+) ([\d\/:\s]+)", map: &["amount", "sender", "trxid", "balance", "datetime"] },
        ],
        "rocket" => &[
            Format { type_: "Personal", priority: 100, pattern: r"Tk([\d,.]+) received from A\/C:([*\d]+) Fee:Tk([\d,.]+)\, Your A\/C Balance: Tk([\d,.]+) TxnId:([A-Z0-9]+)(?: Date:([\w\-:\s]+))?", map: &["amount", "sender", "fee", "balance", "trxid", "datetime"] },
        ],
        "upay" => &[
            Format { type_: "Personal", priority: 100, pattern: r"Tk\. ([\d,.]+) has been received from (\d+)\.(?:\s*Ref[:\-]?\s*(\S+))? Balance Tk\. ([\d,.]+)\. TrxID ([A-Z0-9]+) at ([\d\/:\s]+)\.", map: &["amount", "sender", "ref", "balance", "trxid", "datetime"] },
        ],
        "tap" => &[
            Format { type_: "Personal", priority: 100, pattern: r"Received Tk ([\d,.]+) from (\d+)\. Balance Tk\. ([\d,.]+)\. TxID: ([A-Z0-9]+)\.", map: &["amount", "sender", "balance", "trxid"] },
        ],
        "cellfin" => &[
            Format { type_: "Personal", priority: 100, pattern: r"Islami Bank CellFin Received ([\d,.]+) Tk From CellFin: (\d+) To CellFin: (\d+) TrxId: ([A-Z0-9]+)", map: &["amount", "sender", "receiver", "trxid"] },
        ],
        "okwallet" => &[
            Format { type_: "Personal", priority: 100, pattern: r"\(OK Wallet\) Successfully received Tk ([\d,.]+) from A\/C (\d+)\.(?:\s*Ref[:\-]?\s*(\S+))? Balance Tk ([\d,.]+)\. TrxID ([A-Z0-9]+)", map: &["amount", "sender", "ref", "balance", "trxid"] },
        ],
        "mcash" => &[
            Format { type_: "Personal", priority: 100, pattern: r"IBBL mCash You have received Tk: ([\d,.]+) From: (\d+)(?:\s*Reference:\s*(\S*))? Balance Tk: ([\d,.]+) TrxID: ([A-Z0-9]+)", map: &["amount", "sender", "ref", "balance", "trxid"] },
        ],
        "pathaopay" => &[
            Format { type_: "Personal", priority: 100, pattern: r"You have received BDT ([\d,.]+) from (\+?\d+)\. Balance BDT ([\d,.]+) TrxID ([A-Z0-9]+)", map: &["amount", "sender", "balance", "trxid"] },
        ],
        _ => &[],
    }
}

/// Port of `MFSMessageVerified($mfs, $message)`: returns `None` when no
/// pattern matches, exactly like PHP returning `false`.
pub fn parse_message(provider_key: &str, message: &str) -> Option<ParsedSms> {
    let message = message.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut list: Vec<&Format> = formats(provider_key).iter().collect();
    list.sort_by(|a, b| b.priority.cmp(&a.priority));
    for format in list {
        let re = Regex::new(&format!("(?i){}", format.pattern)).expect("valid SMS regex");
        let Some(caps) = re.captures(&message) else {
            continue;
        };
        let mut amount = String::new();
        let mut balance: Option<String> = None;
        let mut sender = String::new();
        let mut trxid = String::new();
        let mut datetime: Option<String> = None;
        for (i, key) in format.map.iter().enumerate() {
            let value = caps
                .get(i + 1)
                .map(|m| m.as_str().replace(',', ""))
                .unwrap_or_default();
            match *key {
                "amount" => amount = value,
                "balance" => balance = Some(value),
                "sender" => sender = value,
                "trxid" => trxid = value,
                "datetime" => {
                    let v = value.trim().to_string();
                    if !v.is_empty() {
                        datetime = Some(v);
                    }
                }
                _ => {}
            }
        }
        return Some(ParsedSms {
            type_: format.type_.to_string(),
            amount_minor: parse_money(&amount),
            amount,
            balance_minor: balance.as_deref().map(parse_money),
            balance,
            sender,
            trxid,
            datetime,
        });
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bkash_personal() {
        let p = parse_message(
            "bkash",
            "You have received Tk 500.00 from 01712345678. Ref: 10293847. Fee Tk 0.00. Balance Tk 15,230.00. TrxID BK8M9X2K7P at 23/11/2026 14:32",
        )
        .unwrap();
        assert_eq!(p.type_, "Personal");
        assert_eq!(p.amount, "500.00");
        assert_eq!(p.amount_minor, 50_000_000_000);
        assert_eq!(p.sender, "01712345678");
        assert_eq!(p.balance.as_deref(), Some("15230.00"));
        assert_eq!(p.trxid, "BK8M9X2K7P");
    }

    #[test]
    fn bkash_cash_in() {
        let p = parse_message(
            "bkash",
            "Cash In Tk 1000.00 from 01898765432 successful. Fee Tk 5.00. Balance Tk 10500.00. TrxID BK8M9X2K8Q at 23/11/2026 10:00",
        )
        .unwrap();
        assert_eq!(p.amount_minor, 100_000_000_000);
        assert_eq!(p.trxid, "BK8M9X2K8Q");
    }

    #[test]
    fn nagad_personal() {
        let p = parse_message(
            "nagad",
            "Money Received. Amount: Tk 1,000.00 Sender: 01812345678 TxnID: NGD123456789 Balance: Tk 24,500.00 23/11/2026 15:00",
        )
        .unwrap();
        assert_eq!(p.amount_minor, 100_000_000_000);
        assert_eq!(p.balance.as_deref(), Some("24500.00"));
        assert_eq!(p.trxid, "NGD123456789");
    }

    #[test]
    fn rocket_personal() {
        let p = parse_message(
            "rocket",
            "Tk300.00 received from A/C:01912345678 Fee:Tk0.00, Your A/C Balance: Tk5000.00 TxnId:RKT123456 Date:23-Nov-2026 12:00",
        )
        .unwrap();
        assert_eq!(p.amount_minor, 30_000_000_000);
        assert_eq!(p.trxid, "RKT123456");
    }

    #[test]
    fn upay_and_tap() {
        let u = parse_message(
            "upay",
            "Tk. 300.00 has been received from 01912345678. Balance Tk. 5500.00. TrxID UPAY123456 at 23/11/2026 12:00.",
        )
        .unwrap();
        assert_eq!(u.amount_minor, 30_000_000_000);
        let t = parse_message(
            "tap",
            "Received Tk 250.00 from 01712345678. Balance Tk. 3250.00. TxID: TAP123456.",
        )
        .unwrap();
        assert_eq!(t.amount_minor, 25_000_000_000);
    }

    #[test]
    fn cellfin_no_balance() {
        let p = parse_message(
            "cellfin",
            "Islami Bank CellFin Received 500.00 Tk From CellFin: 01712345678 To CellFin: 01812345678 TrxId: CF123456",
        )
        .unwrap();
        assert_eq!(p.balance, None);
        assert_eq!(p.trxid, "CF123456");
    }

    #[test]
    fn unknown_message_returns_none() {
        assert!(parse_message("bkash", "Hello, this is not a payment SMS").is_none());
        // telecash / ipay have no parse regex in PHP
        assert!(parse_message("telecash", "anything").is_none());
    }

    #[test]
    fn money_parsing() {
        assert_eq!(parse_money("15,230.00"), 1_523_000_000_000);
        assert_eq!(parse_money("0.00"), 0);
        assert_eq!(parse_money("abc"), 0);
        assert_eq!(money_to_string(1_523_000_000_000), "15230.00");
    }

    #[test]
    fn whitelist_exact_match() {
        assert_eq!(sender_whitelist("bkash").map(|p| p.0), Some("bkash"));
        assert_eq!(sender_whitelist("16216").map(|p| p.0), Some("rocket"));
        assert_eq!(sender_whitelist("BKASH").map(|p| p.0), Some("bkash"));
        assert_eq!(sender_whitelist("16167"), None);
    }
}
