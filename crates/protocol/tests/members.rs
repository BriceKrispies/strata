//! The members request: its operations, and that a redemption carries no address.

use serde_json::json;
use strata_protocol::api::MembersRequest;

/// Verifies: ADM-153
///
/// A redemption names the invitation and nothing else: a body that adds an address, whatever it
/// calls it, is not a members request at all, so the address the caller would like to be
/// redeemed with never reaches the backend.
#[test]
fn a_redemption_carries_no_address_the_caller_controls() {
    let plain: MembersRequest =
        serde_json::from_value(json!({ "op": "redeem", "invitation": "inv-1" })).unwrap();
    assert_eq!(
        plain,
        MembersRequest::Redeem {
            invitation: "inv-1".into()
        }
    );
    for field in ["address", "verified_address", "email"] {
        let body = json!({ "op": "redeem", "invitation": "inv-1", field: "me@example.org" });
        assert!(
            serde_json::from_value::<MembersRequest>(body).is_err(),
            "{field} was accepted"
        );
    }
}
