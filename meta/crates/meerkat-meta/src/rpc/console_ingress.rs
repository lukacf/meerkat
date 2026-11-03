//! Console ingress handler for admin REST API forwarding.

use super::*;

pub fn handle_console_ingress_json(decisions: &RuntimeDecisionState, request_json: &str) -> String {
    let request: ConsoleRestJsonRequest = match serde_json::from_str(request_json) {
        Ok(request) => request,
        Err(_) => {
            let response = ConsoleRestJsonResponse {
                status: 400,
                body: serde_json::json!({"error":"invalid_request"}),
            };
            return serde_json::to_string(&response).unwrap_or_else(|_| {
                r#"{"status":500,"body":{"error":"internal_error"}}"#.to_string()
            });
        }
    };
    let response = handle_console_rest_json_route(decisions, &request);
    serde_json::to_string(&response)
        .unwrap_or_else(|_| r#"{"status":500,"body":{"error":"internal_error"}}"#.to_string())
}
