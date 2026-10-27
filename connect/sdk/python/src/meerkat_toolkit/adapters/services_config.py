"""Inert configuration and tool declarations for selected service operations."""

from __future__ import annotations

from copy import deepcopy
from dataclasses import dataclass
import json
import math
import os
from pathlib import Path
import re
import stat
from types import MappingProxyType
from typing import Any, Mapping


OPERATIONS = {
    "slack": ("auth_test", "list_conversations_page", "get_conversation", "history_page", "replies_page", "read_interval", "get_permalink", "get_user"),
    "gmail": ("list_messages_page", "get_message", "send_raw", "create_draft_raw", "send_message", "get_profile", "read_message", "read_text_part", "create_draft_message"),
    "google-calendar": ("list_events_page", "get_event", "insert_event", "patch_event", "delete_event"),
    "google-drive": ("list_files_page", "get_file", "download_content", "export_content"),
}
WRITES = frozenset({"send_raw", "create_draft_raw", "send_message", "create_draft_message", "insert_event", "patch_event", "delete_event"})
ORIGINS = {"slack": "https://slack.com/", "gmail": "https://gmail.googleapis.com/",
           "google-calendar": "https://www.googleapis.com/", "google-drive": "https://www.googleapis.com/"}

# Guidance uses the existing library prerequisite vocabulary. It is not an
# authentication recipe executor or a claim about the selected account.
_AUTH_PREREQUISITES = {
    "google-drive": (
        ("account", "Select the Google account and authorize its accessible files for this task. "
         "The host supplies that account's token; file IDs and search queries do not verify identity "
         "or restrict the account to a folder. No impersonation or per-file grant is established here."),
        ("client_authentication", "config.token_env names a host-supplied Google OAuth access token. "
         "It is sent only to https://www.googleapis.com/. This is a local stdio MCP server. "
         "An API key, refresh token or client secret is not an access token."),
        ("registration", "Enable the Google Drive API and select the host's registered OAuth client, "
         "consent and protected callback flow. The adapter does not perform login or register an app. "
         "Workspace administrator restrictions and applicable OAuth verification remain independent. "
         "Reference: https://developers.google.com/workspace/drive/api/guides/api-specific-auth"),
        ("permissions", "Metadata-only list_files_page/get_file can use "
         "https://www.googleapis.com/auth/drive.metadata.readonly. For download_content/export_content, "
         "use https://www.googleapis.com/auth/drive.readonly, or the narrower per-file "
         "https://www.googleapis.com/auth/drive.file for files already shared with this app. "
         "drive.file also permits provider writes, but this profile exposes only reads; metadata-only "
         "authorization cannot download content. Scope does not override file access or download restrictions. "
         "No scope is requested or broadened automatically. Checked 2026-10-04: "
         "https://developers.google.com/workspace/drive/api/guides/api-specific-auth ; "
         "https://developers.google.com/workspace/drive/api/reference/rest/v3/files/get ; "
         "https://developers.google.com/workspace/drive/api/reference/rest/v3/files/export"),
        ("resource_access", "Search/list returns one page with nextPageToken and incompleteSearch. "
         "Continue explicitly with the same query; incompleteSearch=true means some results are missing. "
         "No token only ends that query, not all accessible files or drives. Explicit shared-drive "
         "corpora, shortcut following, revision snapshots, automatic retries and writes are not exposed. "
         "Read metadata before choosing blob download or a Workspace export MIME type. Content larger "
         "than max_content_bytes fails without partial output. Binary or non-UTF-8 data is retained "
         "as an MCP base64 blob resource; this is not document extraction or a model-readability guarantee."),
    ),
    "google-calendar": (
        ("account", "Select the Google account and one calendar for the task. config.calendar_id "
         "fixes request routing for every operation; primary means the selected token's primary calendar. "
         "Neither the ID nor the alias verifies account identity or grants access."),
        ("client_authentication", "config.token_env names a host-supplied environment variable containing "
         "a Google OAuth access token. The local connector sends it only to https://www.googleapis.com/. "
         "The local MCP transport is stdio. An API key, refresh token or client secret is not an access token."),
        ("registration", "Enable the Google Calendar API in the selected Google Cloud project. Use the "
         "host's existing Google OAuth client, consent and protected callback flow to obtain the selected "
         "account's access token. Registration, consent and administrator restrictions remain host concerns; "
         "this component does not register an app or perform login. "
         "The packaged host-only Google reconnect/refresh helpers accept service=calendar with "
         "documented Calendar scopes; the host still owns known-account consent and credentials. "
         "Reference: https://developers.google.com/workspace/calendar/api/quickstart/python"),
        ("permissions", "For list_events_page and get_event, the read-only event scope is "
         "https://www.googleapis.com/auth/calendar.events.readonly. This does not grant access to a "
         "calendar the account cannot read. insert_event, patch_event and delete_event require write "
         "authorization, such as https://www.googleapis.com/auth/calendar.events, and access to modify "
         "the selected calendar. calendar.events.owned or calendar.app.created may apply to their "
         "restricted calendar sets. Select the narrow scope appropriate to the host's intended "
         "resources; no scope is requested or broadened automatically. "
         "Checked 2026-10-04: https://developers.google.com/workspace/calendar/api/auth ; "
         "https://developers.google.com/workspace/calendar/api/v3/reference/events/list ; "
         "https://developers.google.com/workspace/calendar/api/v3/reference/events/get ; "
         "https://developers.google.com/workspace/calendar/api/v3/reference/events/insert ; "
         "https://developers.google.com/workspace/calendar/api/v3/reference/events/patch ; "
         "https://developers.google.com/workspace/calendar/api/v3/reference/events/delete"),
        ("resource_access", "Only the configured calendar is available. Listing returns one page; "
         "nextPageToken means coverage is incomplete, even for an empty page. Continue explicitly with "
         "the same query and returned page_token. No token only ends that chosen query, not all calendars "
         "or all updates. Provider access roles may hide event details. Preserve time zones, all-day dates, "
         "recurrence and cancelled tombstones; do not infer missing content. No sync-token consumption, "
         "calendar discovery or automatic traversal is exposed. Writes require explicit operation "
         "selection and host authorization of the event and affected attendees. Notification query "
         "options are not exposed; provider defaults apply and some emails may still be sent. "
         "A write error may follow a committed change; never automatically retry or replace an event."),
    ),
    "slack": (
        ("account", "Select the Slack account and workspace whose conversations the task may access. "
         "The host must verify that identity and the selected token's access before activation."),
        ("client_authentication", "config.token_env names a host-supplied environment variable containing "
         "a Slack Web API access token. The local connector sends that bearer token only to "
         "https://slack.com/. This is downstream service authentication; the local MCP transport is stdio."),
        ("registration", "Create or select a Slack app for the intended workspace. In OAuth & Permissions, "
         "add only the Bot Token Scopes or User Token Scopes needed below, then install or reinstall the app "
         "and approve those scopes. Use the resulting bot or user access token through the host credential "
         "owner, not an app signing secret. If workspace app approval is enabled, a Workspace Owner or "
         "app manager must approve the request; installation is not universally an administrator-only step. "
         "Checked 2026-10-03: https://docs.slack.dev/authentication/installing-with-oauth/ ; "
         "https://slack.com/help/articles/222386767-Manage-app-approval-for-your-workspace"),
        ("permissions", "For list_conversations_page (conversations.list) and get_conversation "
         "(conversations.info), choose scopes by the conversation kinds needed: channels:read for public "
         "channels, groups:read for private channels, im:read for direct messages, and mpim:read for "
         "group direct messages. Do not request all four solely because this profile exposes all kinds. "
         "Checked 2026-10-03: https://docs.slack.dev/reference/methods/conversations.list/ ; "
         "https://docs.slack.dev/reference/methods/conversations.info/"),
        ("permissions", "For history_page and replies_page, choose the corresponding channels:history, "
         "groups:history, im:history or mpim:history scope. read_interval uses both conversations.history "
         "and conversations.replies, so its selected token must support both for that conversation. "
         "Current method references list bot and user tokens. Bot history access requires conversation "
         "membership; user history access includes public conversations and private conversations the "
         "user belongs to. A scope or channel ID does not add membership or override resource access. "
         "Checked 2026-10-03: https://docs.slack.dev/reference/methods/conversations.history/ ; "
         "https://docs.slack.dev/reference/methods/conversations.replies/"),
        ("permissions", "auth_test (auth.test) and get_permalink (chat.getPermalink) require a valid "
         "access token but no additional OAuth scope. A permalink does not establish that a recipient "
         "can open the conversation. These operations do not require chat:write. "
         "Checked 2026-10-03: https://docs.slack.dev/reference/methods/auth.test/ ; "
         "https://docs.slack.dev/reference/methods/chat.getPermalink/"),
        ("permissions", "Optional get_user (users.info) reads one explicitly selected user's workspace "
         "profile and requires users:read for bot or user tokens. The host must authorize this separate "
         "metadata access; the connector does not request or widen scopes. Names do not require "
         "users:read.email, and email fields may be absent. Profile fields may be absent, null or empty. "
         "A profile observation does not establish channel membership or audience access. "
         "Checked 2026-10-04: https://docs.slack.dev/reference/methods/users.info/"),
        ("resource_access", "Check the selected workspace's message retention and the app's rate tier "
         "before promising an interval is complete. New commercially distributed non-Marketplace apps "
         "and installations have a 1 request/minute, 15-object limit for history/replies; current Slack "
         "guidance exempts existing installations and keeps Tier 3 for Marketplace and internal "
         "customer-built apps. read_interval may need multiple requests and returns explicit partial "
         "coverage on limits or refusal; configuration bounds do not increase Slack quotas. "
         "Checked 2026-10-03: https://docs.slack.dev/reference/methods/conversations.history/ ; "
         "https://docs.slack.dev/reference/methods/conversations.replies/"),
        ("resource_access", "The host can set config.allowed_channel_ids to an array of up to 256 unique "
         "exact channel IDs. An empty array refuses every channel call; omitting the field leaves channel "
         "routing unrestricted by this adapter. A configured ceiling also refuses list_conversations_page "
         "rather than fetching broad metadata and filtering it. auth_test and get_user remain available "
         "if explicitly selected; get_user is independent workspace metadata, not channel-scoped access. "
         "This fixed routing limit does not verify the workspace, grant access, or filter quoted message content."),
    ),
    "gmail": (
        ("account", "Select the Gmail account. config.user_id selects API request routing, either me or "
         "an explicit mailbox; it does not verify the token's account or grant access to that mailbox."),
        ("client_authentication", "config.token_env names a host-supplied environment variable containing "
         "a Google access token authorized for the selected Gmail operations. The local connector sends "
         "that bearer token only to https://gmail.googleapis.com/. This is downstream service "
         "authentication; the local MCP transport is stdio."),
        ("registration", "For user-authorized Gmail access, use a Google account with the intended "
         "mailbox, create or select a Google Cloud project, and enable the Gmail API. Configure Google "
         "Auth platform Branding, Audience and Data Access; add test users while an external app is in "
         "Testing. Create an OAuth client matching the host's existing callback flow: Desktop app for "
         "a desktop flow, or Web application with its registered redirect URI for a web server flow. "
         "The host obtains consent and supplies the resulting access token. An API key, OAuth client "
         "secret or refresh token is not the access token expected by config.token_env. "
         "Checked 2026-10-03: https://developers.google.com/workspace/gmail/api/quickstart/python ; "
         "https://developers.google.com/workspace/guides/configure-oauth-consent ; "
         "https://developers.google.com/workspace/guides/create-credentials"),
        ("permissions", "For list_messages_page and get_message with general search or message content, "
         "request https://www.googleapis.com/auth/gmail.readonly. A narrower "
         "https://www.googleapis.com/auth/gmail.metadata token can list without a query and get only "
         "metadata or minimal format; Gmail disallows the list query and full/raw formats with that "
         "scope. get_message defaults to full, so explicitly select metadata or minimal when using "
         "gmail.metadata. Metadata access is not message-body access. "
         "Optional get_profile observes the selected mailbox's emailAddress for host comparison. "
         "It accepts gmail.metadata, gmail.readonly, gmail.compose, gmail.modify or https://mail.google.com/. "
         "gmail.send alone cannot call get_profile. Do not broaden permissions or use a fallback automatically; "
         "select this operation only when the host has authorized a suitable scope. The observation is not "
         "a stable issuer/subject identity, granted-scope inventory or send-as verification. "
         "Profile scope source checked 2026-10-04: https://developers.google.com/workspace/gmail/api/reference/rest/v1/users/getProfile . "
         "Checked 2026-10-03: https://developers.google.com/workspace/gmail/api/reference/rest/v1/users.messages/list ; "
         "https://developers.google.com/workspace/gmail/api/reference/rest/v1/users.messages/get ; "
         "https://developers.google.com/workspace/gmail/api/reference/rest/v1/Format"),
        ("permissions", "For send_raw or send_message alone, the narrow grant is https://www.googleapis.com/auth/gmail.send. "
         "For create_draft_raw or create_draft_message, use https://www.googleapis.com/auth/gmail.compose; gmail.send does not "
         "authorize draft creation. gmail.compose also permits sending, but neither grant provides "
         "general inbox reads. If reads are selected too, add the applicable read scope above. "
         "Broader gmail.modify or https://mail.google.com/ grants are not required by these operations. "
         "Provider scopes do not select tools or approve a particular send or draft. "
         "Checked 2026-10-03: https://developers.google.com/workspace/gmail/api/reference/rest/v1/users.messages/send ; "
         "https://developers.google.com/workspace/gmail/api/reference/rest/v1/users.drafts/create"),
        ("resource_access", "Google classifies gmail.send as sensitive and gmail.readonly, gmail.metadata "
         "and gmail.compose as restricted. The app operator must check applicable verification and "
         "security-assessment requirements before production distribution; internal and personal-use "
         "exceptions are conditional. Workspace administrators can block requested scopes or apps "
         "independently of user consent. An internal app is limited to its organization; changing "
         "config.user_id does not impersonate another mailbox. Domain-wide delegation, if separately "
         "chosen by the host, needs Workspace super-administrator authorization and host-managed "
         "impersonated tokens; this connector does not establish it. "
         "Checked 2026-10-03: https://developers.google.com/workspace/gmail/api/auth/scopes ; "
         "https://developers.google.com/identity/protocols/oauth2/production-readiness/restricted-scope-verification ; "
         "https://developers.google.com/workspace/guides/configure-oauth-consent ; "
         "https://developers.google.com/identity/protocols/oauth2/native-app ; "
         "https://developers.google.com/workspace/guides/create-credentials"),
        ("application_binding", "send_raw and create_draft_raw accept canonical standard base64 of "
         "complete, host-authorized MIME bytes, including all To/Cc/Bcc recipients and content. They "
         "do not compose a message or ask for approval. send_message composes plain-text MIME from "
         "an explicit from_address, to recipients, subject and body_text, with optional cc and bcc "
         "recipients. create_draft_message uses the same explicit fields and composition to create "
         "one draft without sending it. It must be selected separately; it does not enable sending "
         "or inbox reading. The sender is not inferred from config.user_id or verified by composition. "
         "Keep the existing host write-approval boundary "
         "and treat an uncertain result as uncertain, not permission to resend. Credential acquisition, "
         "message composition and approval are separate from this local configuration form."),
        ("resource_access", "Optional read_message and read_text_part require full-content read "
         "permission such as gmail.readonly; gmail.metadata cannot read full bodies. read_message "
         "makes one request and decodes bounded inline outer text with explicit omitted parts. "
         "read_text_part re-reads the message and may fetch exactly the selected external text part, "
         "using at most two reads under one configured deadline. HTML is unrendered source, "
         "attachments remain labelled, and nested messages never supply outer sender/content. "
         "No URL fetch, scope escalation, retry or sender authentication is inferred. Checked "
         "2026-10-04: https://developers.google.com/workspace/gmail/api/reference/rest/v1/Format ; "
         "https://developers.google.com/workspace/gmail/api/reference/rest/v1/users.messages.attachments/get"),
        ("resource_access", "The host can set config.allowed_recipient_addresses to an array of up to "
         "256 unique bare mailbox addresses for send_message and create_draft_message. An empty "
         "array refuses every structured send or draft; "
         "omitting the field leaves recipients unrestricted by this adapter. Only ASCII domain case "
         "is normalized; local parts and non-ASCII domains remain exact, without alias or wildcard "
         "matching. All To/Cc/Bcc recipients must match. This configuration refuses send_raw and "
         "create_draft_raw because opaque MIME cannot preserve this boundary. It does not approve "
         "a send, verify sender identity, or restrict the direct raw SDK APIs."),
    ),
}
_HOST_PREREQUISITE = (
    "application_binding", "The host launches the local MCP process for one selected account and "
    "operation subset and injects only its selected credential reference. Obtain or refresh credentials "
    "through the existing credential owner and protected human surface when needed. This component "
    "does not perform OAuth, open an authentication page, acquire or refresh a token. It reads the "
    "selected value once at serve startup; credential rotation requires a replacement process. "
    "Keep secret values out of configuration, command arguments and model context."
)
_UNRESOLVED_AUTH = (
    "The host-selected account's existing authorization, any developer registration or administrator "
    "approval, and the selected token's actually granted provider scopes are not established by config-help. Resolve them for the "
    "selected operations through the provider and credential owner before activation.",
    "A valid configuration or available token does not establish account identity, permitted resources, "
    "a grant, connectivity or readiness. This guidance grants no authority.",
)


CONFIGURATION_DIAGNOSTIC_PATHS = MappingProxyType({
    "invalid_document": ((),),
    "invalid_config_file": ((),),
    "invalid_profile": (("profile",),),
    "invalid_operations": (("operations",),),
    "invalid_config_fields": (("config",),),
    "invalid_channel_ids": (("config", "allowed_channel_ids"),),
    "channel_list_conflict": (("operations",),),
    "invalid_recipient_addresses": (("config", "allowed_recipient_addresses"),),
    "recipient_operation_conflict": (("operations",),),
    "invalid_credential_reference": (("config", "token_env"),),
    "integer_out_of_range": tuple(("config", name) for name in (
        "timeout_seconds", "max_response_bytes", "max_message_bytes", "max_content_bytes",
        "max_interval_requests", "max_interval_messages", "max_interval_result_bytes")),
    "invalid_mailbox": (("config", "user_id"),),
    "invalid_calendar_id": (("config", "calendar_id"),),
})


class ConfigurationError(ValueError):
    def __init__(self, code: str | None = None, field_path: tuple[str, ...] = ()) -> None:
        super().__init__("Invalid service MCP configuration")
        if code is not None and (
            type(code) is not str or type(field_path) is not tuple
            or field_path not in CONFIGURATION_DIAGNOSTIC_PATHS.get(code, ())
        ):
            raise ValueError("Invalid configuration diagnostic")
        self._code, self._field_path = code, field_path if code is not None else ()

    @property
    def diagnostic(self) -> dict[str, Any] | None:
        """Only fixed validator vocabulary; no supplied values or unknown keys."""
        if self._code is None:
            return None
        return {"version": 1, "code": self._code, "field_path": list(self._field_path)}


@dataclass(frozen=True)
class Configuration:
    profile: str
    operations: tuple[str, ...]
    config: Mapping[str, Any]


def validate_configuration(value: Any) -> Configuration:
    invalid = False
    failure = None
    try:
        if type(value) is not dict or set(value) != {"profile", "operations", "config"}:
            raise ConfigurationError("invalid_document")
        profile, operations, config = value["profile"], value["operations"], value["config"]
        if type(profile) is not str or profile not in OPERATIONS:
            raise ConfigurationError("invalid_profile", ("profile",))
        if type(operations) is not list or not operations:
            raise ConfigurationError("invalid_operations", ("operations",))
        if any(type(item) is not str or item not in OPERATIONS[profile] for item in operations):
            raise ConfigurationError("invalid_operations", ("operations",))
        if len(set(operations)) != len(operations):
            raise ConfigurationError("invalid_operations", ("operations",))
        if type(config) is not dict:
            raise ConfigurationError("invalid_config_fields", ("config",))
        required = {"token_env", "timeout_seconds", "max_response_bytes"}
        if profile == "gmail":
            required |= {"user_id", "max_message_bytes"}
        if profile == "google-calendar":
            required.add("calendar_id")
        if profile == "google-drive":
            required.add("max_content_bytes")
        if "read_interval" in operations:
            required |= {"max_interval_requests", "max_interval_messages"}
        optional = {"slack": {"allowed_channel_ids"}, "gmail": {"allowed_recipient_addresses"},
                    "google-calendar": set(), "google-drive": set()}[profile]
        if "read_interval" in operations:
            optional.add("max_interval_result_bytes")
        if not required <= set(config) or not set(config) <= required | optional:
            raise ConfigurationError("invalid_config_fields", ("config",))
        frozen_config = dict(config)
        if "allowed_channel_ids" in config:
            channels = config["allowed_channel_ids"]
            if (type(channels) is not list or len(channels) > 256
                    or any(type(item) is not str or not re.fullmatch(r"[A-Za-z0-9]{1,128}", item)
                           for item in channels)
                    or len(set(channels)) != len(channels)):
                raise ConfigurationError("invalid_channel_ids", ("config", "allowed_channel_ids"))
            if "list_conversations_page" in operations:
                raise ConfigurationError("channel_list_conflict", ("operations",))
            frozen_config["allowed_channel_ids"] = tuple(channels)
        if "allowed_recipient_addresses" in config:
            from .gmail_composition import normalize_recipient_addresses

            if {"send_raw", "create_draft_raw"}.intersection(operations):
                raise ConfigurationError("recipient_operation_conflict", ("operations",))
            bad_recipients = False
            try:
                frozen_config["allowed_recipient_addresses"] = normalize_recipient_addresses(
                    config["allowed_recipient_addresses"])
            except ValueError:
                bad_recipients = True
            if bad_recipients:
                raise ConfigurationError("invalid_recipient_addresses", ("config", "allowed_recipient_addresses"))
        reference = config["token_env"]
        if type(reference) is not str or not re.fullmatch(r"[A-Za-z_][A-Za-z0-9_]{0,127}", reference):
            raise ConfigurationError("invalid_credential_reference", ("config", "token_env"))
        bounds = {"timeout_seconds": (1, 60), "max_response_bytes": (1, 16_777_216),
                  "max_message_bytes": (1, 4_194_304), "max_interval_requests": (1, 100),
                  "max_interval_messages": (1, 10_000), "max_content_bytes": (1, 16_777_216),
                  "max_interval_result_bytes": (4096, 16_777_216)}
        for field, (minimum, maximum) in bounds.items():
            if field in config and (type(config[field]) is not int or not minimum <= config[field] <= maximum):
                raise ConfigurationError("integer_out_of_range", ("config", field))
        if profile == "gmail":
            user = config["user_id"]
            if (type(user) is not str or len(user) > 320
                    or (user != "me" and not re.fullmatch(r"[A-Za-z0-9._+-]+@[A-Za-z0-9.-]+", user))):
                raise ConfigurationError("invalid_mailbox", ("config", "user_id"))
        if profile == "google-calendar":
            calendar = config["calendar_id"]
            valid = type(calendar) is str and bool(calendar) and calendar not in {".", ".."}
            if valid:
                valid = not any(ord(char) < 32 or ord(char) == 127 for char in calendar)
                try:
                    valid = valid and len(calendar.encode("utf-8")) <= 8192
                except UnicodeError:
                    valid = False
            if not valid:
                raise ConfigurationError("invalid_calendar_id", ("config", "calendar_id"))
        result = Configuration(profile, tuple(operations), MappingProxyType(frozen_config))
    except ConfigurationError as error:
        invalid, failure = True, error.diagnostic
    except Exception:
        invalid = True
    if invalid:
        if failure is not None:
            raise ConfigurationError(failure["code"], tuple(failure["field_path"]))
        raise ConfigurationError()
    return result


def _object(pairs: list[tuple[str, Any]]) -> dict[str, Any]:
    result: dict[str, Any] = {}
    for key, value in pairs:
        if key in result:
            raise ValueError()
        result[key] = value
    return result


def _number(value: str) -> float:
    number = float(value)
    if not math.isfinite(number):
        raise ValueError()
    return number


def _constant(_value: str) -> Any:
    raise ValueError()


def load_configuration(path: str | Path) -> Configuration:
    invalid = False
    known_file_error = False
    try:
        descriptor = os.open(path, os.O_RDONLY | os.O_NONBLOCK)
        with os.fdopen(descriptor, "rb") as source:
            if not stat.S_ISREG(os.fstat(source.fileno()).st_mode):
                raise ValueError()
            data = source.read(65_537)
        if len(data) > 65_536:
            raise ValueError()
        value = json.loads(data.decode("utf-8"), object_pairs_hook=_object,
                           parse_float=_number, parse_constant=_constant)
    except (OSError, ValueError, TypeError):
        invalid, known_file_error = True, True
    except Exception:
        invalid = True
    if invalid:
        if known_file_error:
            raise ConfigurationError("invalid_config_file")
        raise ConfigurationError()
    return validate_configuration(value)


def profiles() -> dict[str, Any]:
    return {"profiles": [{"profile": profile, "operations": [
        {"operation": name, "tool": f"{profile}_{name}", "effect": "write" if name in WRITES else "read"}
        for name in operations]} for profile, operations in OPERATIONS.items()]}


def configuration_help(profile: str) -> dict[str, Any]:
    if type(profile) is not str or profile not in OPERATIONS:
        raise ConfigurationError()
    fields = {
        "token_env": {"kind": "string", "credential_reference": True},
        "timeout_seconds": {"kind": "integer", "minimum": 1, "maximum": 60},
        "max_response_bytes": {"kind": "integer", "minimum": 1, "maximum": 16_777_216},
    }
    if profile == "gmail":
        fields |= {"user_id": {"kind": "string"},
                   "max_message_bytes": {"kind": "integer", "minimum": 1, "maximum": 4_194_304},
                   "allowed_recipient_addresses": {"kind": "string_list", "required": False}}
    elif profile == "slack":
        fields |= {"allowed_channel_ids": {"kind": "string_list", "required": False},
                   "max_interval_requests": {"kind": "integer", "minimum": 1, "maximum": 100,
                                            "only_when_operation_selected": "read_interval"},
                   "max_interval_messages": {"kind": "integer", "minimum": 1, "maximum": 10_000,
                                            "only_when_operation_selected": "read_interval"},
                   "max_interval_result_bytes": {"kind": "integer", "minimum": 4096,
                       "maximum": 16_777_216, "default": 262144, "required": False,
                       "only_when_operation_selected": "read_interval",
                       "description": "Full compact ASCII JSON result bytes, including continuation and coverage metadata."}}
    elif profile == "google-calendar":
        fields["calendar_id"] = {"kind": "string"}
    else:
        fields["max_content_bytes"] = {"kind": "integer", "minimum": 1, "maximum": 16_777_216}
    return {"profile": profile, "operations": next(row["operations"] for row in profiles()["profiles"]
                                                     if row["profile"] == profile),
            "top_level_fields": ["profile", "operations", "config"], "config_fields": fields,
            "prerequisites": [{"kind": kind, "instruction": instruction}
                              for kind, instruction in (*_AUTH_PREREQUISITES[profile], _HOST_PREREQUISITE)],
            "unresolved_details": [*_UNRESOLVED_AUTH, *(
                ["Sending Gmail messages and creating drafts require separate host operation approval; "
                 "provider authorization alone does not approve either effect."] if profile == "gmail" else
                ["Calendar writes require explicit host operation approval; scope possession does not approve "
                 "event content, attendees or an unconditional patch/delete. A provider receipt is not "
                 "proof of notification delivery."] if profile == "google-calendar" else [])],
            "scope": "Explicit nonempty operation subset. Applicable fields are required unless marked optional; other fields are refused. "
                     "Field help is not a complete JSON Schema. Read README.md for semantics. Credentials, grants and "
                     "account readiness belong to the host; check-config and describe do not read credentials."}


def _fields(properties: dict[str, Any], required: tuple[str, ...] = ()) -> dict[str, Any]:
    return {"type": "object", "properties": properties, "required": list(required), "additionalProperties": False}


def tool_descriptions(configuration: Configuration) -> list[dict[str, Any]]:
    boolean = {"type": "boolean"}
    identifier = {"type": "string", "minLength": 1, "maxLength": 128, "pattern": r"^[A-Za-z0-9]+$"}
    timestamp = {"type": "string", "minLength": 1, "maxLength": 64, "pattern": r"^[0-9]+(?:\.[0-9]+)?$"}
    text = {"type": "string", "minLength": 1, "maxLength": 8192}
    nullable = lambda schema: {"anyOf": [schema, {"type": "null"}]}
    page = {"channel": identifier, "oldest": nullable(timestamp), "latest": nullable(timestamp),
            "inclusive": boolean, "cursor": nullable(text), "limit": {"type": "integer", "minimum": 1, "maximum": 999},
            "include_all_metadata": boolean}
    gmail_id = {"type": "string", "minLength": 1, "maxLength": 1024, "pattern": r"^[A-Za-z0-9_-]+$"}
    if configuration.profile == "slack":
        schemas = {
            "auth_test": _fields({}),
            "get_user": _fields({"user": identifier, "include_locale": boolean}, ("user",)),
            "list_conversations_page": _fields({
                "types": {"type": "array", "minItems": 1, "maxItems": 4, "uniqueItems": True,
                          "items": {"enum": ["public_channel", "private_channel", "mpim", "im"]}},
                "exclude_archived": boolean, "cursor": nullable(text),
                "limit": {"type": "integer", "minimum": 1, "maximum": 999}, "team_id": nullable(identifier)}),
            "get_conversation": _fields({"channel": identifier, "include_locale": boolean,
                                         "include_num_members": boolean}, ("channel",)),
            "get_permalink": _fields({"channel": identifier, "message_ts": timestamp},
                                      ("channel", "message_ts")),
            "history_page": _fields(page, ("channel",)),
            "replies_page": _fields({**page, "ts": timestamp}, ("channel", "ts")),
            "read_interval": _fields({"channel": identifier, "start": timestamp, "end": timestamp,
                "root_oldest": timestamp, "max_requests": {"type": "integer", "minimum": 1,
                    "maximum": configuration.config.get("max_interval_requests", 1)},
                "max_messages": {"type": "integer", "minimum": 1,
                    "maximum": configuration.config.get("max_interval_messages", 1)},
                "max_result_bytes": {"type": "integer", "minimum": 4096,
                    "maximum": configuration.config.get("max_interval_result_bytes", 262144),
                    "default": configuration.config.get("max_interval_result_bytes", 262144),
                    "description": "Maximum bytes of the complete compact ASCII JSON result. A result_budget stop preserves the refused page for an explicit resume; never treat it as empty or complete data."},
                "page_limit": {"type": "integer", "minimum": 1, "maximum": 999},
                "include_all_metadata": boolean, "resumable": boolean,
                "continuation": {"anyOf": [{"type": "null"}, _fields({
                    "schema_version": {"const": 1, "type": "integer"},
                    "selection": _fields({"channel": identifier, "start": timestamp,
                        "end": timestamp, "root_oldest": timestamp,
                        "page_limit": {"type": "integer", "minimum": 1, "maximum": 999},
                        "include_all_metadata": boolean},
                        ("channel", "start", "end", "root_oldest", "page_limit", "include_all_metadata")),
                    "pending": {"type": "array", "minItems": 1, "maxItems": 1025,
                        "items": _fields({"root_ts": nullable(timestamp), "cursor": nullable(text),
                            "seen_cursors": {"type": "array", "maxItems": 2048,
                                "uniqueItems": True, "items": text}},
                            ("root_ts", "cursor", "seen_cursors"))},
                    "discovered_roots": {"type": "array", "maxItems": 1024,
                        "uniqueItems": True, "items": timestamp}},
                    ("schema_version", "selection", "pending", "discovered_roots"))]}},
                ("channel", "start", "end", "root_oldest", "max_requests", "max_messages")),
        }
    elif configuration.profile == "google-drive":
        file_id = {"type": "string", "minLength": 1, "maxLength": 8192,
                   "description": "Explicit Drive file ID, not a URL. Does not follow shortcuts."}
        schemas = {
            "list_files_page": _fields({"query": nullable(text), "page_token": nullable(text),
                "order_by": nullable(text), "page_size": {"type": "integer", "minimum": 1, "maximum": 1000}}),
            "get_file": _fields({"file_id": file_id}, ("file_id",)),
            "download_content": _fields({"file_id": file_id}, ("file_id",)),
            "export_content": _fields({"file_id": file_id, "mime_type": {
                "type": "string", "minLength": 3, "maxLength": 256,
                "pattern": r"^[A-Za-z0-9!#$&^_.+-]+/[A-Za-z0-9!#$&^_.+-]+$",
                "description": "Explicit supported Workspace export media type, for example text/plain or application/pdf."}},
                ("file_id", "mime_type")),
        }
    elif configuration.profile == "google-calendar":
        event_body = {"type": "object", "additionalProperties": True,
                      "description": "Exact Calendar event JSON. Preserve omitted, null, false and empty values. "
                                     "Serialized request body is limited to 2097152 bytes. The provider validates "
                                     "event fields; unsupported query options are not inferred from this body."}
        etag = nullable({**text, "description": "One exact quoted provider ETag for If-Match. "
                        "Omission or null explicitly requests an unconditional write. Wildcards and lists are refused."})
        schemas = {
            "list_events_page": _fields({
                "time_min": nullable({**text, "description": "RFC3339 with offset. Exclusive lower bound on event end time."}),
                "time_max": nullable({**text, "description": "RFC3339 with offset. Exclusive upper bound on event start time; later than time_min."}),
                "q": nullable(text), "page_token": nullable(text),
                "max_results": {"type": "integer", "minimum": 1, "maximum": 2500},
                "single_events": boolean,
                "order_by": {"enum": ["startTime", "updated"], "description": "startTime requires single_events=true."},
                "show_deleted": boolean,
            }),
            "get_event": _fields({"event_id": text}, ("event_id",)),
            "insert_event": _fields({"event": event_body}, ("event",)),
            "patch_event": _fields({"event_id": text, "patch": event_body, "expected_etag": etag},
                                   ("event_id", "patch")),
            "delete_event": _fields({"event_id": text, "expected_etag": etag}, ("event_id",)),
        }
    else:
        message_limit = configuration.config["max_message_bytes"]
        encoded_limit = 4 * ((message_limit + 2) // 3)
        raw = {"mime_base64": {"type": "string", "minLength": 4, "maxLength": encoded_limit,
                              "description": "Canonical standard base64 of exact host-authorized MIME bytes, including all To/Cc/Bcc recipients and content."},
               "thread_id": nullable(gmail_id)}
        mailbox = {"type": "string", "minLength": 1, "maxLength": message_limit,
                   "description": "One explicit mailbox, optionally with a display name. Header line breaks and malformed addresses are refused."}
        recipients = {"type": "array", "maxItems": message_limit, "items": mailbox}
        structured_message = _fields({
            "from_address": mailbox,
            "to": {**recipients, "minItems": 1}, "cc": recipients, "bcc": recipients,
            "subject": {"type": "string", "maxLength": message_limit,
                        "description": "Plain subject text without header line breaks."},
            "body_text": {"type": "string", "maxLength": message_limit,
                          "description": "Plain Unicode text. SMTP serialization normalizes line endings and adds a final text newline."},
        }, ("from_address", "to", "subject", "body_text"))
        schemas = {
            "get_profile": _fields({}),
            "list_messages_page": _fields({"query": nullable(text),
                "label_ids": {"type": "array", "maxItems": 100, "items": gmail_id},
                "page_token": nullable(text), "max_results": {"type": "integer", "minimum": 1, "maximum": 500},
                "include_spam_trash": boolean}),
            "get_message": _fields({"message_id": gmail_id, "format": {"enum": ["full", "metadata", "minimal", "raw"]},
                "metadata_headers": {"type": "array", "maxItems": 100, "items": {"type": "string", "minLength": 1, "maxLength": 998}}}, ("message_id",)),
            "read_message": _fields({"message_id": gmail_id}, ("message_id",)),
            "read_text_part": _fields({"message_id": gmail_id, "part_id": {
                "type": "string", "maxLength": 1024, "pattern": r"^[A-Za-z0-9_.-]*$",
                "description": "Exact provider partId from read_message, including empty string for the root. Not an attachment ID or a URL."}}, ("message_id", "part_id")),
            "send_raw": _fields(raw, ("mime_base64",)),
            "create_draft_raw": _fields(raw, ("mime_base64",)),
            "send_message": structured_message,
            "create_draft_message": structured_message,
        }
    notes = {
        "list_files_page": "Read one Drive search page with a fixed metadata mask including nextPageToken and incompleteSearch. Query uses Drive q syntax; include trashed = false if desired. Pass a returned page_token explicitly with the same query. incompleteSearch=true means missing results; no token only ends this query. No automatic pagination, all-drive coverage or shortcut following.",
        "get_file": "Read fixed Drive metadata including id, name, description, mimeType, modifiedTime, size, webViewLink, shortcutDetails and capabilities. Preserve returned provider values and inspect canDownload before choosing content access. No shortcut following, revision snapshot or account attestation.",
        "download_content": "Download one Drive blob (alt=media) within the host max_content_bytes cap. Workspace documents require an explicit export instead. Return exact UTF-8 textual content as an MCP text resource when compact; binary, non-UTF-8 and escape-heavy text use exact base64 blob resources. Binary is not extracted text and model support varies. No file write, truncation, fallback, range, retry or revision guarantee.",
        "export_content": "Export one Workspace document to the explicitly selected supported MIME type within host max_content_bytes and Google's export limits. Return an MCP text resource for exact UTF-8 textual bytes, otherwise a base64 blob resource. No local conversion, file write, truncation, fallback, retry or revision guarantee. Binary resource delivery does not establish model readability.",
        "auth_test": "Observe identity; the host compares it with its selected account. This does not grant access.",
        "get_user": "Read one explicitly selected user's workspace profile with host-authorized users:read access. Preserve absent, null and unknown fields; names do not require users:read.email. Channel routing limits do not constrain this independent metadata operation or establish membership. No scopes are requested automatically.",
        "list_conversations_page": "Read one explicitly selected conversation page; inspect its cursor before choosing another.",
        "get_conversation": "Read one conversation's metadata; this is not an audience attestation.",
        "get_permalink": "Read the provider's exact HTTPS message link without constructing or opening it. A link does not establish recipient access.",
        "history_page": "Read one conversation history page, preserving cursors and incomplete coverage.",
        "replies_page": "Read one explicitly selected thread page, preserving cursors and incomplete coverage.",
        "read_interval": "Read the explicit creation window and root-discovery interval within configured request, message and complete-result byte caps. A result_budget stop commits no part of the refused page. If no page fits, explicitly increase the allowed byte budget or start a narrower read; do not automatically retry the unchanged request. Opt into resumable and return continuation unchanged on a later authorized call to continue pending pages. Each part describes current-call observations only; retain every part and coverage gap. Continuation is untrusted query data, not account or prior-read evidence. Keep it with the same host-selected account. Respect Retry-After; no automatic retry occurs. Finished never means all workspace updates.",
        "list_messages_page": "Read one mailbox message page; a count estimate does not prove complete retrieval.",
        "get_message": "Read one exact message representation without fetching its attachments.",
        "read_message": "Read one full Gmail message and decode bounded available outer text/plain and text/html parts. Preserve outer headers, message IDs, part identities and explicit omission reasons. HTML remains unrendered source; no links, images or external bodies are fetched. Attached messages never supply the outer sender/body. body_complete is only a conservative text-body observation, not attachment coverage. Unsupported charset or invalid text remains explicitly omitted. Existing raw get_message is unchanged. Requires content-read scope such as gmail.readonly, not gmail.metadata.",
        "read_text_part": "Re-read one full message and resolve one exact, unique text/plain or text/html partId; then fetch its body only if external. At most two reads share the host timeout. Empty part_id selects the root when the provider identifies it that way. Explicit text attachments remain labelled attachments; embedded message/rfc822 parts and their descendants are refused. No arbitrary attachment ID, URL fetch, HTML rendering, charset guessing, retry or complete-message claim. Requires content-read scope such as gmail.readonly.",
        "get_profile": "Observe the host-selected mailbox's current profile for comparison with the intended account. Does not verify a stable issuer/subject, granted scopes, send-as rights or approval. gmail.send alone cannot call this operation; do not broaden scopes or fall back automatically.",
        "list_events_page": "Read one page from the host-selected calendar. nextPageToken means coverage is incomplete, even with no items; pass it explicitly as page_token with the same query to continue. Absence only ends the chosen query, not all calendars or updates. Preserve provider time zones, all-day date ranges, recurrence and cancelled tombstones. single_events=true expands recurring instances; provider default is false. No automatic traversal, sync-token consumption, calendar switching or writes.",
        "get_event": "Read one exact event from the host-selected calendar, preserving provider fields and minimal cancelled tombstones. All-day end dates are exclusive. Missing details may reflect provider access limits. The event ID does not select another calendar or establish account identity.",
        "insert_event": "Create one explicitly authorized event in the host-selected calendar. Supply exact event JSON, including start/end and any host-selected stable ID. No time, attendees or ID is invented. All-day end dates are exclusive. Notification options are unavailable and provider defaults may send email. Errors after connector entry are uncertain; never automatically retry or create a replacement event. A successful receipt is not notification delivery.",
        "patch_event": "Patch one explicitly authorized event in the host-selected calendar. Omitted fields are unchanged; arrays replace the existing array. Supply the exact quoted expected_etag for a conditional write; omission or null is explicitly unconditional. A conflict never triggers automatic read/overwrite. Preserve time zones, all-day dates and recurrence. Notification options are unavailable; provider defaults may send email. Errors after connector entry are uncertain; never automatically retry.",
        "delete_event": "Delete one explicitly authorized event from the host-selected calendar. Supply the exact quoted expected_etag for a conditional write; omission or null is explicitly unconditional. Success returns null after an empty HTTP 204 receipt. Notification options are unavailable; provider defaults may send email. Errors after connector entry are uncertain; a conflict or missing event never becomes success or triggers automatic retry.",
        "send_raw": "Send exact MIME once. The host must authorize all decoded recipients and content. Failure after entry can be unknown; never automatically retry. A provider receipt is not recipient delivery.",
        "create_draft_raw": "Create one draft from exact authorized MIME without sending it. Failure after entry can be unknown; never automatically retry.",
        "send_message": "Compose bounded plain-text SMTPUTF8 MIME and send it once. The host must authorize the explicit sender, all To/Cc/Bcc recipients and content. Composition does not verify sender identity or grant approval. Failure after entry can be unknown; never automatically retry. A provider receipt is not recipient delivery.",
        "create_draft_message": "Compose bounded plain-text SMTPUTF8 MIME and create one draft without sending it. The host must authorize the explicit sender, all To/Cc/Bcc recipients and content. Composition does not verify sender identity or grant approval. Requires draft permission such as gmail.compose; gmail.send alone is insufficient. Failure after entry can be unknown; never automatically retry. A draft receipt does not mean an email was sent.",
    }
    if configuration.profile == "slack" and "allowed_channel_ids" in configuration.config:
        for name in ("get_conversation", "get_permalink", "history_page", "replies_page", "read_interval"):
            notes[name] += " Channel routing is limited by the host's fixed allowed_channel_ids configuration."
    if configuration.profile == "gmail" and "allowed_recipient_addresses" in configuration.config:
        for name in ("send_message", "create_draft_message"):
            notes[name] += " Every To/Cc/Bcc recipient must be within the host's fixed allowed_recipient_addresses configuration."
    return [{"name": f"{configuration.profile}_{name}", "operation": name,
             "effect": "write" if name in WRITES else "read", "description": notes[name],
             "inputSchema": deepcopy(schemas[name])} for name in configuration.operations]
