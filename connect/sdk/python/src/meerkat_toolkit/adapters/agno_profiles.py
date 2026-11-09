"""Explicit, bounded service profiles for the experimental Agno bridge.

Configuration parsing is inert. Upstream modules are imported only when the
host deliberately calls build_profile, including after CLI stdio ownership.
"""

from __future__ import annotations

from collections.abc import Mapping, Sequence
from contextlib import ExitStack
from copy import deepcopy
from dataclasses import dataclass, field
import json
import math
import os
from pathlib import Path
import re
from typing import TYPE_CHECKING, Any
from urllib.parse import urlsplit

from .agno_guidance import authentication_metadata, configuration_prerequisites, selected_configuration_guide

if TYPE_CHECKING:
    from .agno import AgnoToolkitAdapter


class ProfileConfigError(ValueError):
    def __init__(self) -> None:
        super().__init__("Invalid Agno profile configuration")


@dataclass(frozen=True)
class ProfileConfiguration:
    name: str
    operations: tuple[str, ...]
    config: dict[str, Any]


@dataclass(frozen=True)
class AgnoMcpProfile:
    name: str
    adapter: AgnoToolkitAdapter
    descriptions: tuple[dict[str, Any], ...]
    thread_handoff_safe: bool = False
    error_prefixes: dict[str, tuple[str, ...]] = field(default_factory=dict)
    _resources: ExitStack | None = field(default=None, repr=False, compare=False)
    json_error_operations: frozenset[str] = field(default_factory=frozenset)

    def close(self) -> None:
        """Close owned resources after all servers/calls using this profile join.

        The profile creator owns this call. It is idempotent, but is not a
        cancellation primitive and must never race an active worker.
        """
        if self._resources is not None:
            self._resources.close()


_OPERATIONS = {
    "hackernews": ("get_top_hackernews_stories", "get_user_details"),
    "pubmed": ("search_pubmed",),
    "searxng": ("search_web", "news_search"),
    "tavily": ("web_search_using_tavily",),
    "financial-datasets": ("get_income_statements", "get_balance_sheets"),
    "openweather": ("get_current_weather", "get_forecast", "get_air_pollution", "geocode_location"),
    "bitbucket": ("list_repositories", "get_repository_details", "list_all_pull_requests",
                  "get_pull_request_details"),
    "gitlab": ("list_projects", "get_project", "list_merge_requests", "get_merge_request", "list_issues"),
    "clickup": ("get_task", "list_spaces"),
    "discord": ("get_channel_info", "get_channel_messages", "list_channels"),
    "shopify": ("get_shop_info", "get_products"),
    "spotify": ("get_current_user", "get_my_top_tracks", "get_user_playlists", "search_tracks"),
    "unsplash": ("search_photos", "get_photo"),
    "calcom": ("get_available_slots", "get_upcoming_bookings"),
    "jina": ("read_url", "search_query"),
    "openrouteservice": ("geocode_location", "get_directions"),
    "zendesk": ("search_zendesk",),
    "bright-data": ("scrape_as_markdown", "search_engine"),
    "linkup": ("web_search_with_linkup",),
    "todoist": ("get_task",),
    "zoom": ("list_meetings", "get_meeting"),
    "exa": ("search_exa",),
    "firecrawl": ("scrape_website", "map_website"),
    "trello": ("list_boards",),
    "salesforce": ("get_record",),
    "twilio": ("get_call_details",),
    "webex": ("list_rooms",),
    "plivo": ("get_call_details",),
    "arxiv": ("search_arxiv_and_return_articles",),
    "reddit": ("get_top_posts", "get_subreddit_stats", "get_user_info"),
    "x": ("get_user_info", "search_posts"),
    "youtube": ("get_youtube_video_data",),
    "oxylabs": ("search_google", "get_amazon_product"),
    "wikipedia": ("search_wikipedia",),
    "aws-lambda": ("list_functions",),
    "postgres": ("show_tables", "describe_table"),
}

_SERVICE_PROFILES = {"clickup", "discord", "shopify", "spotify", "unsplash"}
_RESEARCH_PROFILES = {"calcom", "jina", "openrouteservice", "zendesk"}
_NEXT_SERVICE_PROFILES = {"bright-data", "linkup", "todoist", "zoom"}
_LIBRARY_PROFILES = {"exa", "firecrawl", "trello", "salesforce", "twilio", "webex", "plivo", "arxiv"}
_HTTP_READ_PROFILES = {"reddit", "x", "youtube", "oxylabs", "wikipedia", "aws-lambda"}
_KEY_PROFILES = {"tavily", "financial-datasets", "openweather", "bitbucket", "gitlab"} | _SERVICE_PROFILES | _RESEARCH_PROFILES | _NEXT_SERVICE_PROFILES | (_LIBRARY_PROFILES - {"arxiv"}) | (_HTTP_READ_PROFILES - {"youtube", "wikipedia"})


def _origin(value: Any) -> str:
    if type(value) is not str or not 1 <= len(value) <= 2048:
        raise ValueError()
    if any(ord(c) <= 32 or ord(c) >= 127 for c in value) or "\\" in value:
        raise ValueError()
    parsed = urlsplit(value)
    if (parsed.scheme != "https" or not parsed.hostname or parsed.username is not None
            or parsed.password is not None or parsed.path not in ("", "/")
            or parsed.query or parsed.fragment or "%" in parsed.netloc):
        raise ValueError()
    if parsed.port is not None and not 1 <= parsed.port <= 65535:
        raise ValueError()
    return f"https://{parsed.netloc}"


def configuration_fields(name: str) -> list[dict[str, Any]]:
    """Field presence shared by validation and help; semantic checks stay below."""
    if type(name) is not str or name not in _OPERATIONS:
        raise ProfileConfigError()
    required = {"timeout"}
    optional: set[str] = set()
    if name == "pubmed":
        required.add("email")
        optional |= {"max_results", "results_expanded"}
    elif name == "searxng":
        required.add("host")
    elif name == "wikipedia":
        required.add("language")
    elif name == "postgres":
        required |= {"socket_directory", "port", "database", "user", "password_env", "schema", "max_rows"}
    elif name in _KEY_PROFILES:
        required.add("api_key_env")
        required |= {
            "bitbucket": {"account_email", "workspace", "repo_slug"},
            "gitlab": {"host"}, "clickup": {"workspace_id"},
            "shopify": {"shop_name", "api_version"}, "spotify": {"market"},
            "calcom": {"event_type_id", "user_timezone"},
            "zendesk": {"subdomain"},
            "bright-data": {"web_unlocker_zone", "serp_zone"},
            "trello": {"token_env"}, "salesforce": {"instance_url"},
            "twilio": {"account_sid"}, "plivo": {"auth_id"},
            "reddit": {"user_agent"}, "oxylabs": {"username"},
            "aws-lambda": {"access_key_id_env", "region"},
        }.get(name, set())
        optional |= {
            "openweather": {"units"}, "jina": {"max_content_length", "search_query_content"},
            "linkup": {"depth", "output_type"}, "x": {"include_post_metrics"},
            "aws-lambda": {"session_token_env"},
            "zendesk": {"auth_mode", "account_email"},
        }.get(name, set())
    integers = {"timeout", "max_results", "port", "max_rows", "event_type_id", "max_content_length"}
    booleans = {"results_expanded", "search_query_content", "include_post_metrics"}
    references = {"api_key_env", "password_env", "token_env", "access_key_id_env", "session_token_env"}
    return [{"name": field, "kind": "integer" if field in integers else "boolean" if field in booleans else "string",
             "required": field in required, "credential_reference": field in references}
            for field in sorted(required | optional)]


def list_profiles() -> dict[str, Any]:
    # These are the exact function keys passed to AgnoToolkitAdapter. That
    # owner checks each donor Function.name and publishes the same raw name.
    return {"profiles": [{"profile": name, "operations": list(operations),
                          "operation_tools": {operation: operation for operation in operations}}
                         for name, operations in _OPERATIONS.items()]}


def configuration_help(name: str) -> dict[str, Any]:
    fields = configuration_fields(name)
    prerequisites, unresolved = configuration_prerequisites(name)
    return {
        "profile": name, "operations": list(_OPERATIONS[name]),
        "operation_tools": {operation: operation for operation in _OPERATIONS[name]},
        **authentication_metadata(name),
        "fields": [{"name": "profile", "kind": "string", "required": True, "credential_reference": False},
                   {"name": "operations", "kind": "string_array", "required": True, "credential_reference": False},
                   {"name": "config", "kind": "object", "required": True, "credential_reference": False}],
        "config_fields": fields,
        "prerequisites": prerequisites,
        "unresolved_details": unresolved,
        "selected_guide": selected_configuration_guide(name),
        "guide": {"package_resource": {"package": __package__, "name": "PROFILE-GUIDE.md"},
                  "standalone_resource": "meerkat_toolkit_agno/PROFILE-GUIDE.md",
                  "source_document": "sdk/python/AGNO-MCP.md"},
        "scope": "Field requirements only, not a complete JSON Schema. Select one or more listed operations. "
                 "The packaged guide describes semantic constraints, defaults and account prerequisites. "
                 "Credential references name environment variables; help and check-config never read their values. "
                 "Configuration validity is not authentication, authorization or readiness.",
    }


def validate_configuration(
    name: str, *, operations: Sequence[str], config: Mapping[str, Any],
) -> ProfileConfiguration:
    invalid = False
    try:
        if type(name) is not str or name not in _OPERATIONS:
            raise ValueError()
        if type(operations) not in (list, tuple) or not operations:
            raise ValueError()
        if any(type(op) is not str or op not in _OPERATIONS[name] for op in operations):
            raise ValueError()
        if len(set(operations)) != len(operations):
            raise ValueError()
        if type(config) is not dict:
            raise ValueError()
        fields = configuration_fields(name)
        allowed = {field["name"] for field in fields}
        required = {field["name"] for field in fields if field["required"]}
        if set(config) - allowed or required - set(config):
            raise ValueError()
        if type(config["timeout"]) is not int or not 1 <= config["timeout"] <= 60:
            raise ValueError()
        settings = dict(config)
        if name == "pubmed":
            email = config.get("email")
            if type(email) is not str or not re.fullmatch(r"[^@\s]{1,64}@[^@\s]{1,189}", email):
                raise ValueError()
            maximum = config.get("max_results", 10)
            if type(maximum) is not int or not 1 <= maximum <= 50:
                raise ValueError()
            expanded = config.get("results_expanded", True)
            if type(expanded) is not bool:
                raise ValueError()
            settings.update(max_results=maximum, results_expanded=expanded)
        elif name == "searxng":
            settings["host"] = _origin(config.get("host"))
        elif name == "wikipedia":
            language = config.get("language")
            if type(language) is not str or not re.fullmatch(r"[a-z]{2,3}(?:-[a-z]{2,8})?", language):
                raise ValueError()
        elif name == "postgres":
            socket_directory = config.get("socket_directory")
            if (type(socket_directory) is not str or not socket_directory.startswith("/")
                    or len(socket_directory.encode("utf-8")) > 80
                    or any(ord(c) < 32 or c in ",\x7f" for c in socket_directory)):
                raise ValueError()
            for key in ("database", "user", "schema"):
                value = config.get(key)
                if type(value) is not str or not 1 <= len(value.encode("utf-8")) <= 63 or "\x00" in value:
                    raise ValueError()
            for key, maximum in (("port", 65535), ("max_rows", 1000)):
                if type(config.get(key)) is not int or not 1 <= config[key] <= maximum:
                    raise ValueError()
            reference = config.get("password_env")
            if (type(reference) is not str or not re.fullmatch(r"[A-Za-z_][A-Za-z0-9_]{0,127}", reference)
                    or reference.startswith("PG")):
                raise ValueError()
        elif name in _KEY_PROFILES:
            reference = config.get("api_key_env")
            if type(reference) is not str or not re.fullmatch(r"[A-Za-z_][A-Za-z0-9_]{0,127}", reference):
                raise ValueError()
            if name == "openweather":
                units = config.get("units", "metric")
                if type(units) is not str or units not in ("standard", "metric", "imperial"):
                    raise ValueError()
                settings["units"] = units
            elif name == "bitbucket":
                email = config.get("account_email")
                if (type(email) is not str or len(email) > 254
                        or not re.fullmatch(r"[A-Za-z0-9.!#$%&'*+/=?^_`{|}~-]+@[A-Za-z0-9.-]+", email)):
                    raise ValueError()
                for key in ("workspace", "repo_slug"):
                    value = config.get(key)
                    if type(value) is not str or not re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9_.-]{0,127}", value):
                        raise ValueError()
            elif name == "gitlab":
                settings["host"] = _origin(config.get("host"))
            elif name == "clickup":
                value = config.get("workspace_id")
                if type(value) is not str or not re.fullmatch(r"[0-9]{1,20}", value):
                    raise ValueError()
            elif name == "shopify":
                shop = config.get("shop_name")
                version = config.get("api_version")
                if (type(shop) is not str or not re.fullmatch(r"[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?", shop)
                        or type(version) is not str or not re.fullmatch(r"20[0-9]{2}-(?:01|04|07|10)", version)):
                    raise ValueError()
            elif name == "spotify":
                market = config.get("market")
                if type(market) is not str or not re.fullmatch(r"[A-Z]{2}", market):
                    raise ValueError()
            elif name == "calcom":
                event = config.get("event_type_id")
                timezone = config.get("user_timezone")
                if (type(event) is not int or not 1 <= event <= 2_147_483_647
                        or type(timezone) is not str or not re.fullmatch(r"[A-Za-z0-9_+/-]{1,64}", timezone)):
                    raise ValueError()
            elif name == "jina":
                maximum = config.get("max_content_length", 10_000)
                content = config.get("search_query_content", True)
                if type(maximum) is not int or not 1 <= maximum <= 100_000 or type(content) is not bool:
                    raise ValueError()
                settings.update(max_content_length=maximum, search_query_content=content)
            elif name == "zendesk":
                site = config.get("subdomain")
                mode = config.get("auth_mode", "api_token")
                if (type(site) is not str or not re.fullmatch(r"[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?", site)
                        or type(mode) is not str or mode not in ("api_token", "oauth_access_token")):
                    raise ValueError()
                if mode == "api_token":
                    email = config.get("account_email")
                    if (type(email) is not str or len(email) > 254
                            or not re.fullmatch(r"[A-Za-z0-9.!#$%&'*+/=?^_`{|}~-]+@[A-Za-z0-9.-]+", email)):
                        raise ValueError()
                elif "account_email" in config:
                    # A bearer token's subject is not selected by an email field.
                    raise ValueError()
            elif name == "bright-data":
                for key in ("web_unlocker_zone", "serp_zone"):
                    value = config.get(key)
                    if type(value) is not str or not re.fullmatch(r"[A-Za-z0-9_-]{1,64}", value):
                        raise ValueError()
            elif name == "linkup":
                depth = config.get("depth", "standard")
                output = config.get("output_type", "searchResults")
                if (type(depth) is not str or depth not in ("standard", "deep")
                        or type(output) is not str or output not in ("searchResults", "sourcedAnswer")):
                    raise ValueError()
                settings.update(depth=depth, output_type=output)
            elif name == "trello":
                reference = config.get("token_env")
                if type(reference) is not str or not re.fullmatch(r"[A-Za-z_][A-Za-z0-9_]{0,127}", reference):
                    raise ValueError()
            elif name == "salesforce":
                selected = urlsplit(_origin(config.get("instance_url")))
                if ":" in selected.hostname:
                    raise ValueError()
                port = "" if selected.port in (None, 443) else f":{selected.port}"
                settings["instance_url"] = f"https://{selected.hostname}{port}"
            elif name == "twilio":
                account = config.get("account_sid")
                if type(account) is not str or not re.fullmatch(r"AC[0-9a-fA-F]{32}", account):
                    raise ValueError()
            elif name == "plivo":
                account = config.get("auth_id")
                if type(account) is not str or not re.fullmatch(r"(?:MA|SA)[A-Za-z0-9]{18}", account):
                    raise ValueError()
            elif name == "reddit":
                agent = config.get("user_agent")
                if type(agent) is not str or not re.fullmatch(r"[\x21-\x7e][\x20-\x7e]{6,199}", agent):
                    raise ValueError()
            elif name == "x":
                metrics = config.get("include_post_metrics", False)
                if type(metrics) is not bool:
                    raise ValueError()
                settings["include_post_metrics"] = metrics
            elif name == "oxylabs":
                username = config.get("username")
                if type(username) is not str or not re.fullmatch(r"[A-Za-z0-9_.@-]{1,128}", username):
                    raise ValueError()
            elif name == "aws-lambda":
                region = config.get("region")
                if type(region) is not str or not re.fullmatch(r"(?:us|eu|ap|sa|ca|me|af|il|mx)-(?:central|north|south|east|west|northeast|southeast)-[1-9]", region):
                    raise ValueError()
                for field in ("access_key_id_env", "session_token_env"):
                    if field == "session_token_env" and field not in config:
                        continue
                    reference = config.get(field)
                    if type(reference) is not str or not re.fullmatch(r"[A-Za-z_][A-Za-z0-9_]{0,127}", reference):
                        raise ValueError()
    except Exception:
        invalid = True
    if invalid:
        raise ProfileConfigError()
    return ProfileConfiguration(name, tuple(operations), settings)


def _explicit_key(selection: ProfileConfiguration, reference: str = "api_key_env") -> str:
    value = os.environ.get(selection.config[reference])
    if type(value) is not str or not 1 <= len(value) <= 4096:
        raise ProfileConfigError()
    if any(not 33 <= ord(char) <= 126 for char in value):
        raise ProfileConfigError()
    return value


def _configure_session(session: Any) -> None:
    from requests.adapters import HTTPAdapter

    session.trust_env = False
    session.proxies.clear()
    session.auth = None
    session.verify = True
    session.max_redirects = 0
    for prefix in ("https://", "http://"):
        session.adapters[prefix].close()
        session.mount(prefix, HTTPAdapter(max_retries=0))


def _object(pairs: list[tuple[str, Any]]) -> dict[str, Any]:
    value: dict[str, Any] = {}
    for key, item in pairs:
        if key in value:
            raise ValueError()
        value[key] = item
    return value


def _number(value: str) -> float:
    result = float(value)
    if not math.isfinite(result):
        raise ValueError()
    return result


def _constant(_value: str) -> Any:
    raise ValueError()


def load_configuration(path: str | Path) -> ProfileConfiguration:
    """Read at most 64 KiB, rejecting duplicates, nonfinite values and extras."""
    invalid = False
    try:
        with Path(path).open("rb") as source:
            data = source.read(65_537)
        if len(data) > 65_536:
            raise ValueError()
        value = json.loads(
            data.decode("utf-8"), object_pairs_hook=_object,
            parse_float=_number, parse_constant=_constant,
        )
        if type(value) is not dict or set(value) != {"profile", "operations", "config"}:
            raise ValueError()
        configuration = validate_configuration(
            value["profile"], operations=value["operations"], config=value["config"],
        )
    except Exception:
        invalid = True
    if invalid:
        raise ProfileConfigError()
    return configuration


def _construct_toolkit(selection: ProfileConfiguration, resources: ExitStack) -> Any:
    name = selection.name
    timeout = selection.config["timeout"]
    if name == "hackernews":
        from agno.tools.hackernews import HackerNewsTools

        return HackerNewsTools(
            timeout=timeout,
            enable_get_top_stories="get_top_hackernews_stories" in selection.operations,
            enable_get_user_details="get_user_details" in selection.operations,
        )
    if name == "pubmed":
        from agno.tools.pubmed import PubmedTools

        return PubmedTools(**selection.config)
    if name == "searxng":
        from agno.tools.searxng import Searxng

        return Searxng(host=selection.config["host"], timeout=timeout)
    if name == "postgres":
        from . import agno_transports as transports

        password = os.environ.get(selection.config["password_env"])
        if (type(password) is not str or not 1 <= len(password.encode("utf-8")) <= 4096
                or "\x00" in password):
            raise ValueError()
        toolkit = transports.build_postgres(
            **{key: value for key, value in selection.config.items() if key != "password_env"},
            password=password,
        )
        resources.callback(toolkit.close)
        return toolkit
    if name in _HTTP_READ_PROFILES:
        from . import agno_transports as transports

        session = transports.ServiceSession()
        resources.callback(session.close)
        _configure_session(session)
        public = {"timeout": timeout, "session": session}
        if name == "youtube":
            return transports.build_youtube(**public)
        if name == "wikipedia":
            return transports.build_wikipedia(**public, language=selection.config["language"])
        common = {**public, "api_key": _explicit_key(selection)}
        if name == "reddit":
            return transports.build_reddit(**common, user_agent=selection.config["user_agent"])
        if name == "x":
            return transports.build_x(**common, include_post_metrics=selection.config["include_post_metrics"])
        if name == "oxylabs":
            return transports.build_oxylabs(**common, username=selection.config["username"])
        return transports.build_aws_lambda(**common, access_key_id=_explicit_key(selection, "access_key_id_env"),
                                          session_token=(_explicit_key(selection, "session_token_env")
                                                         if "session_token_env" in selection.config else None),
                                          region=selection.config["region"], resources=resources)
    if name in _LIBRARY_PROFILES:
        from . import agno_transports as transports

        session = transports.ServiceSession()
        resources.callback(session.close)
        _configure_session(session)
        if name == "arxiv":
            return transports.build_arxiv(timeout=timeout, session=session)
        common = {"api_key": _explicit_key(selection), "timeout": timeout, "session": session}
        if name == "exa":
            return transports.build_exa(**common)
        if name == "firecrawl":
            return transports.build_firecrawl(**common, operations=selection.operations)
        if name == "trello":
            return transports.build_trello(**common, token=_explicit_key(selection, "token_env"))
        if name == "salesforce":
            return transports.build_salesforce(**common, instance_url=selection.config["instance_url"])
        if name == "twilio":
            return transports.build_twilio(**common, account_sid=selection.config["account_sid"])
        if name == "webex":
            return transports.build_webex(**common)
        return transports.build_plivo(**common, auth_id=selection.config["auth_id"], resources=resources)
    key = _explicit_key(selection)
    if name in {"financial-datasets", "openweather", "bitbucket", "gitlab"} | _SERVICE_PROFILES | _RESEARCH_PROFILES | _NEXT_SERVICE_PROFILES:
        import requests

        if name in _SERVICE_PROFILES | _RESEARCH_PROFILES | _NEXT_SERVICE_PROFILES:
            from .agno_transports import ServiceSession

            session = ServiceSession()
        else:
            session = requests.Session()
        resources.callback(session.close)
        _configure_session(session)
        if name in _NEXT_SERVICE_PROFILES:
            from . import agno_transports as transports

            common = {"api_key": key, "timeout": timeout, "session": session}
            if name == "bright-data":
                return transports.build_brightdata(**common, web_unlocker_zone=selection.config["web_unlocker_zone"],
                                                  serp_zone=selection.config["serp_zone"], operations=selection.operations)
            if name == "linkup":
                return transports.build_linkup(**common, depth=selection.config["depth"],
                                               output_type=selection.config["output_type"])
            if name == "todoist":
                return transports.build_todoist(**common, resources=resources)
            return transports.build_zoom(**common, operations=selection.operations)
        if name in _SERVICE_PROFILES:
            from . import agno_transports as transports

            common = {"api_key": key, "timeout": timeout, "session": session}
            if name == "clickup":
                return transports.build_clickup(**common, workspace_id=selection.config["workspace_id"])
            if name == "discord":
                return transports.build_discord(**common, operations=selection.operations)
            if name == "shopify":
                return transports.build_shopify(**common, shop_name=selection.config["shop_name"],
                                                 api_version=selection.config["api_version"])
            if name == "spotify":
                return transports.build_spotify(**common, market=selection.config["market"])
            return transports.build_unsplash(**common, operations=selection.operations)
        if name in _RESEARCH_PROFILES:
            from . import agno_transports as transports

            common = {"api_key": key, "timeout": timeout, "session": session}
            if name == "calcom":
                return transports.build_calcom(**common, event_type_id=selection.config["event_type_id"],
                                                user_timezone=selection.config["user_timezone"],
                                                operations=selection.operations)
            if name == "jina":
                return transports.build_jina(**common, max_content_length=selection.config["max_content_length"],
                                             search_query_content=selection.config["search_query_content"],
                                             operations=selection.operations)
            if name == "openrouteservice":
                return transports.build_openrouteservice(**common, operations=selection.operations)
            return transports.build_zendesk(**common, subdomain=selection.config["subdomain"],
                                            account_email=selection.config.get("account_email"),
                                            auth_mode=selection.config.get("auth_mode", "api_token"))
        if name == "financial-datasets":
            from .agno_transports import BorrowedSessionFinancialDatasetsTools

            return BorrowedSessionFinancialDatasetsTools(key, timeout, session=session)
        from .agno_transports import build_bitbucket, build_gitlab, build_openweather

        if name == "openweather":
            return build_openweather(key, timeout, session=session, units=selection.config["units"],
                                     operations=selection.operations)
        if name == "bitbucket":
            return build_bitbucket(key, timeout, session=session,
                                   account_email=selection.config["account_email"],
                                   workspace=selection.config["workspace"],
                                   repo_slug=selection.config["repo_slug"])
        return build_gitlab(key, timeout, session=session, host=selection.config["host"],
                            operations=selection.operations)
    from agno.tools.tavily import TavilyTools

    # Keep a reference during __init__ so a client assigned before a later
    # construction failure is still closed. No donor runtime is constructed.
    toolkit = TavilyTools.__new__(TavilyTools)
    try:
        TavilyTools.__init__(
            toolkit, api_key=key, api_base_url="https://api.tavily.com",
            enable_search=True, enable_search_context=False, enable_extract=False,
            format="json", include_answer=False, search_depth="basic",
            search_params={"timeout": timeout}, max_tokens=6000,
        )
    finally:
        client = getattr(toolkit, "client", None)
        if client is not None:
            resources.callback(client.close)
    _configure_session(toolkit.client.session)
    toolkit.client.proxies = None
    for headers in (toolkit.client.headers, toolkit.client.session.headers):
        headers.pop("X-Project-ID", None)
        headers.pop("X-Tavily-Orgid", None)
    return toolkit


def _build_profile(selection: ProfileConfiguration, *, toolkit: Any = None) -> AgnoMcpProfile:
    from .agno import AgnoToolkitAdapter

    with ExitStack() as resources:
        if toolkit is None:
            toolkit = _construct_toolkit(selection, resources)
        adapter = AgnoToolkitAdapter(
            toolkit, functions={op: "read" for op in selection.operations},
        )
        descriptions = adapter.describe()
        for description in descriptions:
            properties = description["parameters"]["properties"]
            operation = description["name"]
            if operation == "get_top_hackernews_stories":
                properties["num_stories"].update(minimum=1, maximum=50)
            elif operation == "get_user_details":
                properties["username"].update(
                    minLength=1, maxLength=64, pattern=r"^[A-Za-z0-9_-]+$",
                )
            elif operation in ("get_income_statements", "get_balance_sheets"):
                properties["ticker"].update(
                    minLength=1, maxLength=32, pattern=r"^[A-Za-z0-9][A-Za-z0-9.-]*$",
                )
                properties["period"]["enum"] = ["annual", "quarterly", "ttm"]
                properties["limit"].update(minimum=1, maximum=10)
            elif selection.name == "openweather":
                properties["location"].update(minLength=1, maxLength=256)
                for key in ("days", "limit"):
                    if key in properties:
                        properties[key].update(minimum=1, maximum=5)
            elif selection.name == "bitbucket":
                if operation == "list_repositories":
                    properties["count"].update(minimum=10, maximum=50)
                    description["description"] = "Read the first repository page (10-50 entries) in the configured workspace. Pagination links remain data and are not followed."
                elif operation == "list_all_pull_requests":
                    properties["state"]["enum"] = ["OPEN", "MERGED", "DECLINED", "SUPERSEDED"]
                    description["description"] = "Read only the first pull-request page (at most 20 entries) in the configured repository. Despite the upstream function name, this does not enumerate all pull requests. Pagination links remain data and are not followed."
                elif operation == "get_pull_request_details":
                    properties["pull_request_id"].update(minimum=1, maximum=2_147_483_647)
            elif selection.name == "gitlab":
                if "project_id_or_path" in properties:
                    properties["project_id_or_path"].update(
                        minLength=1, maxLength=256,
                        pattern=r"^[A-Za-z0-9][A-Za-z0-9_.-]*(/[A-Za-z0-9][A-Za-z0-9_.-]*)*$",
                    )
                    properties["project_id_or_path"]["description"] = "Project ID or unencoded group/project path; no URL or percent-encoded input."
                for key in ("search", "source_branch", "target_branch", "author_username",
                            "assignee_username", "labels"):
                    if key in properties:
                        properties[key].update(minLength=1, maxLength=256)
                for key, maximum in (("page", 10_000), ("per_page", 50), ("merge_request_iid", 2_147_483_647)):
                    if key in properties:
                        properties[key].update(minimum=1, maximum=maximum)
                if "state" in properties:
                    properties["state"]["enum"] = (["opened", "closed", "merged", "all"]
                                                   if operation == "list_merge_requests"
                                                   else ["opened", "closed", "all"])
                if "page" in properties:
                    description["description"] = (description["description"] or "") + " Returns only the requested page; does not exhaust pagination."
            elif selection.name in _SERVICE_PROFILES:
                _service_schema(selection.name, description)
            elif selection.name in _RESEARCH_PROFILES:
                _research_schema(selection.name, description)
            elif selection.name in _NEXT_SERVICE_PROFILES:
                _next_service_schema(selection.name, description)
            elif selection.name in _LIBRARY_PROFILES:
                _library_schema(selection.name, description)
            elif selection.name in _HTTP_READ_PROFILES:
                _http_read_schema(selection.name, description)
            elif selection.name == "postgres":
                description["parameters"]["additionalProperties"] = False
                if operation == "describe_table":
                    properties["table"].update(minLength=1, maxLength=63, pattern=r"^[^\x00\ud800-\udfff]+(?![\s\S])")
                description["description"] += (
                    " Selected local Unix-socket PostgreSQL schema only, using SCRAM credentials."
                    " Identifiers must fit 63 UTF-8 bytes. Returns CSV with proper quoting, or the donor's"
                    " empty-result message. At most the configured max_rows (1-1000); excess rows fail."
                    " Only metadata visible to the selected database principal; no arbitrary SQL or exports."
                )
            else:
                properties["query"].update(minLength=1, maxLength=4096)
                maximum = 50 if selection.name == "pubmed" else 5
                properties["max_results"].update(minimum=1, maximum=maximum)
            description["description"] = (description["description"] or "") + (
                " This Toolkit profile applies the additional input limits shown in the schema."
            )
        prefixes = {
            "get_top_hackernews_stories": ("Error fetching stories: ",),
            "get_user_details": ("Error getting user details: ",),
            "search_pubmed": ("Could not fetch articles. Error: ",),
            "search_web": ("Error fetching results from searxng: ",),
            "news_search": ("Error fetching results from searxng: ",),
            "get_income_statements": ("API key not set", "Error making request to "),
            "get_balance_sheets": ("API key not set", "Error making request to "),
            "web_search_using_tavily": (),
            "get_channel_info": ("Error getting channel info: ",),
            "get_channel_messages": ("Error getting messages: ",),
            "list_channels": ("Error listing channels: ",),
            "search_photos": ("Error:", "Error searching Unsplash: "),
            "get_photo": ("Error:", "Error getting photo: "),
            "get_available_slots": ("Error: ",),
            "get_upcoming_bookings": ("Error: ",),
            "scrape_as_markdown": ("Please provide ", "Error scraping URL "),
            "search_engine": ("Please provide ", "Error searching for query ", "Unsupported search engine"),
            "web_search_with_linkup": ("Error: ",),
            "search_exa": ("Error: ",),
            "get_top_posts": ("Error getting top posts: ",),
            "get_subreddit_stats": ("Error getting subreddit stats: ",),
            "get_user_info": ("Error getting user info: ",),
            "list_functions": ("Error listing functions: ",),
        }
        # These fixed implementations require serialized client use. An
        # explicitly borrowed Salesforce client keeps that requirement with
        # its host; the profile does not own its resource or cancellation.
        return AgnoMcpProfile(
            selection.name, adapter, tuple(deepcopy(descriptions)), thread_handoff_safe=True,
            error_prefixes={op: prefixes.get(op, ()) for op in selection.operations},
            _resources=resources.pop_all(),
            json_error_operations=(frozenset(selection.operations)
                                   if selection.name in {"openweather", "bitbucket", "gitlab", "todoist", "zoom"} | _SERVICE_PROFILES | _LIBRARY_PROFILES | _HTTP_READ_PROFILES
                                   else frozenset()),
        )


def _service_schema(name: str, description: dict[str, Any]) -> None:
    """Additional MCP limits for the fixed donor operations, not intent matching."""
    properties = description["parameters"]["properties"]
    description["parameters"]["additionalProperties"] = False
    operation = description["name"]
    end = r"(?![\s\S])"
    for key in ("task_id", "photo_id"):
        if key in properties:
            properties[key].update(minLength=1, maxLength=128, pattern=r"^[A-Za-z0-9_-]+" + end)
    for key in ("channel_id", "guild_id"):
        if key in properties:
            properties[key].update(minLength=1, maxLength=20, pattern=r"^[0-9]+" + end)
    if "query" in properties:
        properties["query"].update(minLength=1, maxLength=1024)
    if "limit" in properties:
        properties["limit"].update(minimum=1, maximum=100 if name == "discord" else 50)
    if "max_results" in properties:
        properties["max_results"].update(minimum=1, maximum=50)
    if name == "shopify":
        if "status" in properties:
            properties["status"]["enum"] = ["ACTIVE", "ARCHIVED", "DRAFT"]
            description["description"] = "Read the first product page (1-50 products), with at most the first 10 variants per product. Pagination is not followed."
    elif name == "spotify":
        if operation == "search_tracks":
            properties["max_results"].update(
                maximum=10,
                description="Maximum number of tracks to return (1-10, default 10).",
            )
        if "market" in properties:
            properties["market"].update(minLength=2, maxLength=2, pattern=r"^[A-Z]{2}" + end)
        if "time_range" in properties:
            properties["time_range"]["enum"] = ["short_term", "medium_term", "long_term"]
        if operation != "get_current_user":
            description["description"] = (description["description"] or "") + " Returns only the first page; pagination is not followed."
    elif name == "unsplash":
        if "per_page" in properties:
            properties["per_page"].update(minimum=1, maximum=30)
            properties["page"].update(minimum=1, maximum=10_000)
            properties["orientation"]["enum"] = ["landscape", "portrait", "squarish"]
            properties["color"]["enum"] = ["black_and_white", "black", "white", "yellow", "orange", "red", "purple", "magenta", "green", "teal", "blue"]
        description["description"] = (description["description"] or "") + " Returns metadata and attribution links; does not download image bytes or grant rights to reuse them."


def _research_schema(name: str, description: dict[str, Any]) -> None:
    """Bounds for the explicit source ports; operations are still agent-selected."""
    parameters = description["parameters"]
    parameters["additionalProperties"] = False
    properties = parameters["properties"]
    end = r"(?![\s\S])"
    if name == "calcom":
        for key in ("start_date", "end_date"):
            if key in properties:
                properties[key].update(minLength=10, maxLength=10, pattern=(
                    r"^(?:19[7-9][0-9]|20[0-2][0-9]|203[0-7])-(?:0[1-9]|1[0-2])-(?:0[1-9]|[12][0-9]|3[01])" + end
                ))
        if "email" in properties:
            properties["email"].update(minLength=3, maxLength=254,
                                        pattern=r"^[^@\s]{1,64}@[^@\s]{1,189}" + end)
        if description["name"] == "get_available_slots":
            description["description"] += " Dates are inclusive UTC dates, not local calendar dates; real dates, ordering and the 31-day span are checked before HTTP. Only 1970-2037 is supported."
        else:
            description["description"] += " The configured event type does not filter this operation. Reads the first page of at most 100 bookings and explicitly reports when more are available. No pagination is followed; returned timestamps must be timezone-aware and in 1970-2037."
    elif name == "jina":
        if "url" in properties:
            properties["url"].update(minLength=1, maxLength=4096, pattern=r"^https?://[^\s]+" + end)
        if "query" in properties:
            properties["query"].update(minLength=1, maxLength=1024)
        description["description"] += " Uses the fixed Jina provider, requests JSON and applies the donor's string conversion and character truncation. Plaintext provider responses are errors."
    elif name == "openrouteservice":
        for key in ("location", "start_location", "end_location"):
            if key in properties:
                properties[key].update(minLength=1, maxLength=512)
        if "profile" in properties:
            from agno.tools.openrouteservice import VALID_PROFILES

            properties["profile"]["enum"] = sorted(VALID_PROFILES)
        description["description"] += " Fixed OpenRouteService API; road routing only, no transit or matrix. No provider next URL is followed."
    elif name == "zendesk":
        properties["search_string"].update(minLength=1, maxLength=1024)
        description["description"] += " Returns only the first Help Center article response, with the donor's regex HTML removal. No ticket access or agent frontend is provided."


def _next_service_schema(name: str, description: dict[str, Any]) -> None:
    """Published bounds for the selected SDK and preauthenticated read profiles."""
    parameters = description["parameters"]
    parameters["additionalProperties"] = False
    properties = parameters["properties"]
    end = r"(?![\s\S])"
    if name == "bright-data":
        if "url" in properties:
            properties["url"].update(minLength=1, maxLength=4096, pattern=r"^https?://[^\s]+" + end)
        if "query" in properties:
            properties["query"].update(minLength=1, maxLength=1024)
            properties["engine"]["enum"] = ["google", "bing", "yandex"]
            properties["num_results"].update(minimum=1, maximum=50)
            for key in ("language", "country_code"):
                properties[key].update(minLength=2, maxLength=2, pattern=r"^[A-Za-z]{2}" + end)
            description["description"] += " The donor applies count/language/country hints only to Google; Bing and Yandex ignore them."
        description["description"] += " Returns successful provider Markdown under content. Target URLs go to the fixed Bright Data API; this process does not fetch them directly. No screenshots or dataset jobs."
    elif name == "linkup":
        properties["query"].update(minLength=1, maxLength=1024)
        properties["depth"]["enum"] = ["standard", "deep"]
        properties["output_type"]["enum"] = ["searchResults", "sourcedAnswer"]
        description["description"] += " Returns the real SDK response model as plain JSON. No async research jobs, x402 payments, image downloads or pagination are performed."
    elif name == "todoist":
        properties["task_id"].update(minLength=1, maxLength=128, pattern=r"^[A-Za-z0-9_-]+" + end)
        description["description"] += " Reads one task through the real Todoist SDK. Task/project enumeration and writes are not included."
    elif name == "zoom":
        if "user_id" in properties:
            properties["user_id"].update(minLength=1, maxLength=128, pattern=r"^[A-Za-z0-9_-]+" + end)
            properties["type"]["enum"] = ["scheduled", "live", "upcoming", "previous"]
        if "meeting_id" in properties:
            properties["meeting_id"].update(minLength=1, maxLength=20, pattern=r"^[0-9]+" + end)
        description["description"] += " Uses the selected existing token without acquiring or refreshing one. No recordings, scheduling or deletion."


def _library_schema(name: str, description: dict[str, Any]) -> None:
    parameters = description["parameters"]
    parameters["additionalProperties"] = False
    properties = parameters["properties"]
    end = r"(?![\s\S])"
    if name == "exa":
        properties["query"].update(minLength=1, maxLength=4096)
        properties["num_results"].update(minimum=1, maximum=20)
        properties["category"]["enum"] = ["company", "research paper", "news", "pdf", "github", "tweet", "personal site", "linkedin profile", "financial report"]
        description["description"] += " Uses the real SDK search and result models; no research jobs. Donor text is capped at 10,000 characters per result."
    elif name == "firecrawl":
        properties["url"].update(minLength=1, maxLength=4096, pattern=r"^https?://[^\s]+" + end)
        description["description"] += " Target URLs are data for the fixed Firecrawl API. Markdown scrape or map only; no crawl jobs, browser actions or local URL fetches. Maps over 100 entries fail explicitly."
    elif name == "trello":
        properties["board_filter"]["enum"] = ["all", "open", "closed", "organization", "public", "starred"]
        description["description"] += " Reads through the real Trello SDK; responses over 100 boards fail explicitly. Optional metadata preserves supplied null/false values and is omitted when the provider omits it. No cards or mutations."
    elif name == "salesforce":
        properties["sobject"].update(minLength=1, maxLength=80, pattern=r"^[A-Z][A-Za-z0-9_]*" + end)
        properties["record_id"].update(pattern=r"^[A-Za-z0-9]{15}(?:[A-Za-z0-9]{3})?" + end)
        properties["fields"]["enum"] = [""]
        description["description"] += " Reads one complete record with an existing session token from the configured instance. Omit fields or pass an empty string; SOQL, login and writes are not included."
    elif name == "twilio":
        properties["call_sid"].update(pattern=r"^CA[0-9a-fA-F]{32}" + end)
        description["description"] += " Reads one call in the configured account using its selected auth token. Present timestamps must exactly match Wdy, DD Mon YYYY HH:MM:SS +0000 or GMT, with fixed English tokens, matching weekday and single ASCII spaces; comments, extra content and malformed dates fail. No SMS sending, voice execution, token acquisition, region or edge override."
    elif name == "webex":
        description["description"] += " Complete only for a single provider page of at most 100 rooms. A pagination Link or larger result is an explicit failure, never a partial all-rooms answer. Existing token only."
    elif name == "plivo":
        properties["call_uuid"].update(pattern=r"^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}" + end)
        description["description"] += " Reads one existing call in the configured account. No messaging, voice execution, lookup charges or region fallback."
    elif name == "arxiv":
        properties["query"].update(minLength=1, maxLength=4096)
        properties["num_articles"].update(minimum=1, maximum=20)
        description["description"] += " Reads up to 20 search results through the real arXiv SDK and Atom parser. No PDF downloads or filesystem writes. Malformed or silently skipped provider entries fail."


def build_profile(
    name: str, *, operations: Sequence[str], config: Mapping[str, Any],
) -> AgnoMcpProfile:
    """Build a selected profile; the caller owns close after its workers join.

    Credentialed profiles resolve only their configured api_key_env reference.
    No request is sent. Owned resources also close if construction fails.
    """
    selection = validate_configuration(name, operations=operations, config=config)
    try:
        return _build_profile(selection)
    except Exception:
        pass
    # Do not retain a secret-bearing constructor exception as context.
    raise ProfileConfigError()


def build_salesforce_profile_from_client(
    configuration: ProfileConfiguration, client: Any,
) -> AgnoMcpProfile:
    """Borrow one host-prepared Salesforce client without reading credentials.

    The existing api_key_env reference is validated but never read. The host
    owns current authentication, account binding and the ServiceSession. Use
    this profile with one serialized MCP server, with no concurrent external
    session users or changes to settings/hooks while a worker is active.
    AuthBase and response hooks must support that serialized worker-thread use
    and leave raw-response cleanup with the transport. They must not retarget,
    retry or replay a request. The profile's thread_handoff_safe flag does not
    establish these properties for arbitrary host code. Close the server and
    join its workers before the host closes the client; profile.close never
    closes it.
    Preparation is transport configuration, not native credential admission.
    This constructor is not exposed by the CLI or MCP tools.
    """
    try:
        from .agno_transports import build_salesforce_from_client

        if type(configuration) is not ProfileConfiguration or configuration.name != "salesforce":
            raise ValueError()
        selection = validate_configuration(configuration.name,
            operations=configuration.operations, config=configuration.config)
        toolkit = build_salesforce_from_client(selection.config["timeout"],
            session=client, instance_url=selection.config["instance_url"])
        return _build_profile(selection, toolkit=toolkit)
    except Exception:
        pass
    raise ProfileConfigError()


def _salesforce_native_descriptions(configuration: ProfileConfiguration) -> tuple[dict[str, Any], ...]:
    """Derive the existing donor/schema catalog without a credential or client use."""
    from .agno_transports import ServiceSession, _build_salesforce

    if type(configuration) is not ProfileConfiguration or configuration.name != "salesforce":
        raise ProfileConfigError()
    selection = validate_configuration(configuration.name,
        operations=configuration.operations, config=configuration.config)
    with ServiceSession() as session:
        session.trust_env, session.max_redirects = False, 0
        # No AuthBase is installed. Even accidental dispatch through this
        # catalog-only donor would fail the borrowed transport's preparation
        # check before I/O. Only its existing descriptions leave this scope.
        toolkit = _build_salesforce(None, selection.config["timeout"],
            session=session, instance_url=selection.config["instance_url"])
        profile = _build_profile(selection, toolkit=toolkit)
        try:
            return tuple(deepcopy(profile.descriptions))
        finally:
            profile.close()


def _http_read_schema(name: str, description: dict[str, Any]) -> None:
    parameters = description["parameters"]
    parameters["additionalProperties"] = False
    properties = parameters["properties"]
    end = r"(?![\s\S])"
    if name == "reddit":
        for key in ("username", "subreddit"):
            if key in properties:
                properties[key].update(minLength=1, maxLength=32, pattern=r"^[A-Za-z0-9_-]+" + end)
        if "limit" in properties:
            properties["limit"].update(minimum=1, maximum=25)
            properties["time_filter"]["enum"] = ["hour", "day", "week", "month", "year", "all"]
        description["description"] += " Uses a selected existing token and real PRAW models; no authentication, retries, pagination beyond one response or writes. A short nonterminal page fails instead of silently returning incomplete results."
    elif name == "x":
        if "username" in properties:
            properties["username"].update(minLength=1, maxLength=15, pattern=r"^[A-Za-z0-9_]+" + end)
        if "query" in properties:
            properties["query"].update(minLength=1, maxLength=512)
            properties["max_results"].update(minimum=10, maximum=100)
        description["description"] += " Selected bearer token only. Returns one recent-search page with actual expanded authors; no OAuth acquisition, user-context reads or writes."
    elif name == "youtube":
        properties["url"].update(minLength=1, maxLength=2048, pattern=r"^https://(?:youtu\.be/[A-Za-z0-9_-]{11}|(?:www\.)?youtube\.com/(?:watch\?v=|embed/|v/)[A-Za-z0-9_-]{11})" + end)
        description["description"] += " Exact YouTube watch/embed/v or youtu.be URL with an 11-character ID. The full authority/path/query are checked before I/O. Only public oEmbed metadata, no captions or downloads."
    elif name == "oxylabs":
        if "query" in properties:
            properties["query"].update(minLength=1, maxLength=512, pattern=r"^[\s\S]*\S[\s\S]*" + end)
        if "asin" in properties:
            properties["asin"].update(minLength=10, maxLength=10, pattern=r"^[A-Za-z0-9]{10}" + end)
        properties["domain_code"]["enum"] = ["com", "co.uk", "de", "fr", "it", "es", "ca", "com.au", "co.jp", "in"]
        description["description"] += " Uses the real synchronous Oxylabs SDK; one parsed response only. Google has at most 50 organic results. Provider errors and malformed envelopes fail, with no success fallback."
    elif name == "wikipedia":
        properties["query"].update(minLength=1, maxLength=256, pattern=r"^[^\x00-\x1f\x7f|]+" + end)
        description["description"] += " Exact title in the selected language, without auto-suggestion or knowledge ingestion. At most three API title redirects and 50 disambiguation options; incomplete results fail."
    else:
        description["description"] = "List function names in the selected AWS region using explicit credentials and real botocore serialization, SigV4 and response parsing. At most 50 functions; a continuation marker fails instead of claiming all functions. No invocation, pagination or credential acquisition."
