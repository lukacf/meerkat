"""Narrow transport seams for explicitly selected, reviewed Agno donors.

The request helper is derived from Agno 3.0.11, Apache-2.0 licensed, at
8c3d8ec52b4a13c410fca97d07fbd507790cf29a:
libs/agno/agno/tools/financial_datasets.py (SHA-256
1d57cfe55f095f94deba7828a1c09116aa110581e13e950fed7943c860f378f7).
Other request-helper adaptations at that same commit:
openweather.py SHA-256 48879b103ec86de53697c2cbb495ab97007ac6b2b7917de5ce068559b62f9578;
bitbucket.py SHA-256 2550423ee08a557446cbeda5d3ae6da8c519da2b85ed2a6e9e66d601fa386b5b;
gitlab.py SHA-256 487b529743af8752eaede5c2034bcdf7c0d89ef2a0848b3154fd442bb9b661f7.
Service helper adaptations at that same commit:
clickup.py SHA-256 91ae7bafe1791ab00e9d1bd6998f4140f9db250cb12bba73e83055965ac3fc07;
discord.py SHA-256 9fa239fb2e0c3b318b214c33a53b60c5876952b2668855fad6d3f27bcc022c9b;
shopify.py SHA-256 a5a7eefd2d9fb8923decf9c6eb52bc4d96773a1b5f4b4acf8820097e9e6623d0;
spotify.py SHA-256 9b75e3293ef63a450ae34288ce0561e99389edee6e0593c59e70d7a3d44e2fe8;
unsplash.py SHA-256 e4a89182cbaaa55f512002b047a8fe82e31393f30ac7d46fb2c807b08bc6f1c9.
The operation bodies for those helpers remain inherited without changes. The
Cal.com, Jina, OpenRouteService and Zendesk operation ports are identified below.
The deltas borrow explicit sessions, refuse redirects and omit raw diagnostics.
The five service helpers share Requests transport, close every response and
bound decoded response bytes. Shopify GraphQL errors remain failures.
Fixed factories import their selected optional donor only when invoked.
"""

# Copyright 2025-2026 Agno Inc.
# Licensed under the Apache License, Version 2.0 (the "License");
# you may not use this file except in compliance with the License.
# You may obtain a copy of the License at
# https://www.apache.org/licenses/LICENSE-2.0
# Unless required by applicable law or agreed to in writing, software
# distributed under the License is distributed on an "AS IS" BASIS,
# WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
# See the License for the specific language governing permissions and
# limitations under the License.

from __future__ import annotations

from typing import Any
import base64
from contextlib import ExitStack
from datetime import date, datetime, timezone
import json
import math
import re
import time
from urllib.parse import parse_qs, quote_plus, urlsplit

from agno.tools.financial_datasets import FinancialDatasetsTools
import requests


class ServiceSession(requests.Session):
    """Do not let Requests consume a redirect body to prepare a next request."""

    def resolve_redirects(self, *args: Any, **kwargs: Any) -> Any:
        return iter(())


def _require_salesforce_client(session: requests.Session, origin: str) -> None:
    """Check inert preparation only; the host owns auth and session lifetime."""
    if (not isinstance(session, ServiceSession) or session.trust_env is not False
            or session.verify is not True or session.proxies or session.params
            or session.max_redirects != 0
            or getattr(session.resolve_redirects, "__func__", None) is not ServiceSession.resolve_redirects
            or not isinstance(session.auth, requests.auth.AuthBase)
            or "Authorization" in session.headers):
        raise ValueError("Prepared Salesforce client required")
    adapter = session.get_adapter(origin + "/")
    if (not isinstance(adapter, requests.adapters.HTTPAdapter)
            or adapter.max_retries.total != 0):
        raise ValueError("Prepared Salesforce client required")


def _borrowed_salesforce_bytes(session, method, url, *, timeout, headers,
                               params, body, expected_status, response_headers):
    """Retain actual responses before host hooks, without changing the session."""
    try:
        with ExitStack() as custody:
            deadline = time.monotonic() + timeout
            responses = []

            def own(response):
                if not isinstance(response, requests.Response):
                    raise ValueError()
                if all(response is not previous for previous in responses):
                    responses.append(response)
                    custody.callback(response.close)
                return response

            def check(response):
                if time.monotonic() >= deadline or response.status_code != expected_status:
                    raise ValueError()
                length = response.headers.get("Content-Length")
                if length is not None and int(length) > 1_048_576:
                    raise ValueError()

            def guard(response, *args, **kwargs):
                response = own(response)
                check(response)
                for hook in tuple(session.hooks.get("response", [])):
                    replacement = hook(response, *args, **kwargs)
                    if replacement is not None:
                        response = own(replacement)
                    check(response)
                return response

            try:
                response = own(session.request(method, url, headers=headers, params=params,
                    json=body, timeout=timeout, allow_redirects=False, stream=True,
                    hooks={"response": [guard]}))
            except requests.RequestException as error:
                if isinstance(error.response, requests.Response):
                    own(error.response)
                raise
            check(response)
            response_headers.update(response.headers)
            payload = bytearray()
            for chunk in response.iter_content(16_384):
                if time.monotonic() >= deadline or len(payload) + len(chunk) > 1_048_576:
                    raise ValueError()
                payload.extend(chunk)
            if time.monotonic() >= deadline:
                raise ValueError()
            return bytes(payload)
    except Exception:
        pass
    raise ValueError("Service request failed")


def _service_bytes(
    session: requests.Session, method: str, url: str, *, timeout: int,
    headers: dict[str, str], params: dict[str, Any] | None = None,
    body: dict[str, Any] | None = None, expected_status: int | None = None,
    response_headers: dict[str, str] | None = None,
) -> bytes:
    """One bounded response body; the caller owns the selected session.

    Status is checked before reading. Decoded wire bytes are capped before
    UTF-8 or JSON parsing; every response closes on every path. Socket waits
    and completed slow reads use the same selected timeout as before.
    """
    try:
        deadline = time.monotonic() + timeout
        with session.request(method, url, headers=headers, params=params, json=body,
                             timeout=timeout, allow_redirects=False, stream=True) as response:
            if (not 200 <= response.status_code < 300
                    or (expected_status is not None and response.status_code != expected_status)):
                raise ValueError()
            if response_headers is not None:
                response_headers.update(response.headers)
            limit = 1_048_576
            length = response.headers.get("Content-Length")
            if length is not None and int(length) > limit:
                raise ValueError()
            payload = bytearray()
            for chunk in response.iter_content(16_384):
                if time.monotonic() >= deadline or len(payload) + len(chunk) > limit:
                    raise ValueError()
                payload.extend(chunk)
            if time.monotonic() >= deadline:
                raise ValueError()
            return bytes(payload)
    except Exception as error:
        if isinstance(error, requests.RequestException) and isinstance(error.response, requests.Response):
            try:
                error.response.close()
            except Exception:
                pass
    # Donor methods can log exceptions. Neither the exception context nor its
    # credential-bearing request, headers or provider body may leave this seam.
    raise ValueError("Service request failed")


def _service_json(
    session: requests.Session, method: str, url: str, *, timeout: int,
    headers: dict[str, str], params: dict[str, Any] | None = None,
    body: dict[str, Any] | None = None,
    allow_json_scalar: bool = False,
    expected_status: int | None = None,
) -> Any:
    """Keep the existing strict UTF-8/JSON contract above the byte reader."""
    try:
        payload = _service_bytes(session, method, url, timeout=timeout, headers=headers,
                                 params=params, body=body, expected_status=expected_status)
        value = json.loads(payload.decode("utf-8"), parse_constant=lambda _: _invalid_json())
        if not allow_json_scalar and type(value) not in (dict, list):
            raise ValueError()
        return value
    except Exception:
        pass
    raise ValueError("Service request failed")


def _invalid_json() -> Any:
    raise ValueError()


def build_clickup(api_key: str, timeout: int, *, session: requests.Session, workspace_id: str) -> Any:
    from agno.tools.clickup import ClickUpTools

    class BorrowedClickUpTools(ClickUpTools):
        def _make_request(self, method: str, endpoint: str, params=None, data=None) -> Any:
            if (method != "GET" or params is not None or data is not None or
                    not (endpoint == f"team/{workspace_id}/space" or
                         re.fullmatch(r"task/[A-Za-z0-9_-]{1,128}", endpoint))):
                raise ValueError("Service request failed")
            return _service_json(session, method, f"https://api.clickup.com/api/v2/{endpoint}",
                                 timeout=timeout, headers={"Authorization": api_key})

    return BorrowedClickUpTools(api_key=api_key, master_space_id=workspace_id, timeout=timeout)


def build_discord(api_key: str, timeout: int, *, session: requests.Session, operations: tuple[str, ...]) -> Any:
    from agno.tools.discord import DiscordTools

    class BorrowedDiscordTools(DiscordTools):
        def _make_request(self, method: str, endpoint: str, data=None) -> Any:
            if (method != "GET" or data is not None or not re.fullmatch(
                    r"(?:channels/[0-9]{1,20}(?:/messages\?limit=(?:[1-9][0-9]?|100))?|guilds/[0-9]{1,20}/channels)",
                    endpoint.removeprefix("/"))):
                raise ValueError("Service request failed")
            return _service_json(session, method, f"https://discord.com/api/v10{endpoint}",
                                 timeout=timeout, headers={"Authorization": f"Bot {api_key}"})

    return BorrowedDiscordTools(
        bot_token=api_key, enable_send_message=False, enable_delete_message=False,
        enable_get_channel_info="get_channel_info" in operations,
        enable_get_channel_messages="get_channel_messages" in operations,
        enable_list_channels="list_channels" in operations,
    )


def build_shopify(api_key: str, timeout: int, *, session: requests.Session, shop_name: str, api_version: str) -> Any:
    from agno.tools.shopify import ShopifyTools

    class BorrowedShopifyTools(ShopifyTools):
        def _make_graphql_request(self, query: str, variables=None) -> Any:
            if variables is not None or not query.lstrip().startswith("query {"):
                raise ValueError("Service request failed")
            value = _service_json(
                session, "POST", f"https://{shop_name}.myshopify.com/admin/api/{api_version}/graphql.json",
                timeout=timeout, headers={"X-Shopify-Access-Token": api_key}, body={"query": query},
            )
            if type(value) is not dict or "errors" in value or type(value.get("data")) is not dict:
                raise ValueError("Service request failed")
            return value["data"]

    return BorrowedShopifyTools(shop_name=shop_name, access_token=api_key, api_version=api_version, timeout=timeout)


def build_spotify(api_key: str, timeout: int, *, session: requests.Session, market: str) -> Any:
    from agno.tools.spotify import SpotifyTools

    class BorrowedSpotifyTools(SpotifyTools):
        def _make_request(self, endpoint: str, method: str = "GET", body=None, params=None) -> Any:
            if method != "GET" or body is not None or endpoint not in ("me", "me/top/tracks", "me/playlists", "search"):
                raise ValueError("Service request failed")
            value = _service_json(session, method, f"https://api.spotify.com/v1/{endpoint}", timeout=timeout,
                                  headers={"Authorization": f"Bearer {api_key}"}, params=params)
            if endpoint == "me/playlists" and isinstance(value, dict) and "items" in value:
                # Current Spotify uses items; the pinned donor projects tracks.
                # Missing playlist contents mean unknown count, not an empty list.
                if not isinstance(value["items"], list):
                    raise ValueError("Service request failed")
                playlists = []
                for row in value["items"]:
                    if row is None:
                        playlists.append(None)
                        continue
                    if not isinstance(row, dict):
                        raise ValueError("Service request failed")
                    counts = []
                    for key in ("items", "tracks"):
                        if key in row:
                            container = row[key]
                            if not isinstance(container, dict):
                                raise ValueError("Service request failed")
                            count = container.get("total")
                            if count is not None and (type(count) is not int or count < 0):
                                raise ValueError("Service request failed")
                            counts.append(count)
                    if len(counts) == 2 and counts[0] != counts[1]:
                        raise ValueError("Service request failed")
                    playlists.append({**row, "tracks": {"total": counts[0] if counts else None}})
                value = {**value, "items": playlists}
            return value

    return BorrowedSpotifyTools(access_token=api_key, default_market=market, timeout=timeout)


def build_unsplash(api_key: str, timeout: int, *, session: requests.Session, operations: tuple[str, ...]) -> Any:
    from agno.tools.unsplash import UnsplashTools

    class BorrowedUnsplashTools(UnsplashTools):
        def _make_request(self, endpoint: str, params=None) -> Any:
            if endpoint != "/search/photos" and not re.fullmatch(r"/photos/[A-Za-z0-9_-]{1,128}", endpoint):
                raise ValueError("Service request failed")
            return _service_json(session, "GET", f"https://api.unsplash.com{endpoint}", timeout=timeout,
                                 headers={"Authorization": f"Client-ID {api_key}", "Accept-Version": "v1"}, params=params)

    return BorrowedUnsplashTools(
        access_key=api_key, timeout=timeout, enable_get_random_photo=False, enable_download_photo=False,
        enable_search_photos="search_photos" in operations, enable_get_photo="get_photo" in operations,
    )


# Source ports from the same Agno 3.0.11 commit named above. These donors call
# module-level HTTP directly, so their selected operation bodies are ported
# explicitly instead of patching globals. Real donor Toolkit registration and
# pure parsing/formatting helpers remain in use. Provenance SHA-256:
# calcom.py: 6cc4e8bd4d061a6d0ff14fd89ff825acb444812e6085d2f4613aad3ee22635e5
# jina.py: ae8b552966500ecddf5fb4abfb75f05f50645361e92fb058afb5ff4473371883
# openrouteservice.py: f2529e30b1996cb5f6965dd84f6d90bcbd14cc748fd882d84157fe5611f33014
# zendesk.py: d90078c60a8efa4d21cabddd328ff6bf0558b173414bb3710eba2e30021da821
# I/O uses the selected owned session above, with bounded responses and no
# redirects. Raw diagnostic logging and error bodies are replaced by fixed
# errors. Additional input/date/coordinate checks are documented profile
# deltas, not upstream behavior claims. Jina retains JSON-to-string output.


def build_calcom(api_key: str, timeout: int, *, session: requests.Session,
                 event_type_id: int, user_timezone: str, operations: tuple[str, ...]) -> Any:
    from agno.tools.calcom import CalComTools
    import pytz

    pytz.timezone(user_timezone)

    class SelectedCalComTools(CalComTools):
        def _convert_to_user_timezone(self, utc_time: str) -> str:
            # The inherited helper otherwise interprets naive provider times
            # using the machine timezone. pytz does not promise post-2037 DST.
            instant = datetime.fromisoformat(utc_time.replace("Z", "+00:00"))
            if instant.utcoffset() is None or not 1970 <= instant.astimezone(timezone.utc).year <= 2037:
                raise ValueError("Service request failed")
            formatted = super()._convert_to_user_timezone(utc_time)
            if not 1970 <= int(formatted.split("-", 1)[0]) <= 2037:
                raise ValueError("Service request failed")
            return formatted

        def get_available_slots(self, start_date: str, end_date: str) -> str:
            """Read at most 31 inclusive UTC dates, formatted in the configured timezone."""
            try:
                start, end = date.fromisoformat(start_date), date.fromisoformat(end_date)
                if (start.isoformat() != start_date or end.isoformat() != end_date
                        or not 1970 <= start.year <= end.year <= 2037
                        or not 0 <= (end - start).days <= 30):
                    raise ValueError()
                querystring = {
                    "start": f"{start_date}T00:00:00Z",
                    "end": f"{end_date}T23:59:59Z",
                    "eventTypeId": str(self.event_type_id),
                    # Pin the documented object shape instead of relying on
                    # the provider's ambiguously documented default format.
                    "format": "range", "timeZone": "UTC",
                }
                value = _service_json(session, "GET", "https://api.cal.com/v2/slots",
                                      headers=self._get_headers(api_version="2024-09-04"), params=querystring, timeout=timeout,
                                      expected_status=200)
                if value.get("status") != "success" or type(value.get("data")) is not dict:
                    raise ValueError()
                slots = value["data"]
                available_slots = []
                for date_key, times in slots.items():
                    slot_date = date.fromisoformat(date_key)
                    if (slot_date.isoformat() != date_key or not start <= slot_date <= end
                            or type(times) is not list):
                        raise ValueError()
                    for slot in times:
                        if type(slot) is not dict or type(slot.get("start")) is not str or type(slot.get("end")) is not str:
                            raise ValueError()
                        instant = datetime.fromisoformat(slot["start"].replace("Z", "+00:00"))
                        finish = datetime.fromisoformat(slot["end"].replace("Z", "+00:00"))
                        if (instant.utcoffset() is None or finish.utcoffset() is None
                                or instant.astimezone(timezone.utc).date() != slot_date or finish <= instant):
                            raise ValueError()
                        user_time = self._convert_to_user_timezone(slot["start"])
                        available_slots.append(user_time)
                return f"Available slots: {', '.join(available_slots)}"
            except Exception:
                return "Error: Service request failed"

        def get_upcoming_bookings(self, email: str | None = None) -> str:
            """Read at most 100 upcoming bookings in one page; explicitly report incomplete results."""
            try:
                querystring = {"status": "upcoming", "limit": "100"}
                if email:
                    if not re.fullmatch(r"[^@\s]{1,64}@[^@\s]{1,189}", email):
                        raise ValueError()
                    querystring["attendeeEmail"] = email
                value = _service_json(session, "GET", "https://api.cal.com/v2/bookings",
                                      headers=self._get_headers(api_version="2026-05-01"), params=querystring, timeout=timeout,
                                      expected_status=200)
                bookings, pagination = value.get("data"), value.get("pagination")
                if (value.get("status") != "success" or type(bookings) is not list or len(bookings) > 100
                        or type(pagination) is not dict or type(pagination.get("hasMore")) is not bool
                        or "nextCursor" not in pagination):
                    raise ValueError()
                more, cursor = pagination["hasMore"], pagination["nextCursor"]
                if (more and (type(cursor) is not str or not cursor)
                        or not more and cursor is not None):
                    raise ValueError()
                # Keep the selected one-request contract. A provider cursor is
                # neither a URL to follow nor evidence of complete results.
                incomplete = "\nMore upcoming bookings are available; only the first page is shown." if more else ""
                if not bookings:
                    return ("No upcoming bookings in this page." + incomplete if more
                            else "No upcoming bookings found.")
                booking_info = []
                for booking in bookings:
                    if type(booking) is not dict or any(type(booking.get(key)) is not str for key in ("uid", "title", "start", "status")):
                        raise ValueError()
                    user_time = self._convert_to_user_timezone(booking["start"])
                    booking_info.append(
                        f"uid: {booking['uid']}, Title: {booking['title']}, Time: {user_time}, Status: {booking['status']}"
                    )
                return "Upcoming bookings:\n" + "\n".join(booking_info) + incomplete
            except Exception:
                return "Error: Service request failed"

    return SelectedCalComTools(
        api_key=api_key, event_type_id=event_type_id, user_timezone=user_timezone, timeout=timeout,
        enable_get_available_slots="get_available_slots" in operations,
        enable_get_upcoming_bookings="get_upcoming_bookings" in operations,
        enable_create_booking=False, enable_reschedule_booking=False, enable_cancel_booking=False,
    )


def build_jina(api_key: str, timeout: int, *, session: requests.Session,
               max_content_length: int, search_query_content: bool, operations: tuple[str, ...]) -> Any:
    from agno.tools.jina import JinaReaderTools

    class SelectedJinaReaderTools(JinaReaderTools):
        def read_url(self, url: str) -> str:
            """Read an explicit HTTP(S) URL through Jina and return truncated content."""
            try:
                target = urlsplit(url)
                if (not 1 <= len(url) <= 4096 or target.scheme not in ("http", "https")
                        or not target.hostname or target.username is not None or target.password is not None
                        or any(ord(c) <= 32 for c in url) or "\\" in url
                        or (target.port is not None and not 1 <= target.port <= 65535)):
                    raise ValueError()
                full_url = f"{self.config.base_url}{url}"
                content = _service_json(session, "GET", full_url, headers=self._get_headers(),
                                        timeout=timeout, allow_json_scalar=True)
                return self._truncate_content(str(content))
            except Exception:
                pass
            raise ValueError("Service request failed")

        def search_query(self, query: str) -> str:
            """Search through Jina and return the donor's truncated content format."""
            headers = self._get_headers()
            if not self.config.search_query_content:
                headers["X-Respond-With"] = "no-content"
            body = {"q": query}
            try:
                content = _service_json(session, "POST", str(self.config.search_url), headers=headers,
                                        body=body, timeout=timeout, allow_json_scalar=True)
                return self._truncate_content(str(content))
            except Exception:
                pass
            raise ValueError("Service request failed")

    return SelectedJinaReaderTools(
        api_key=api_key, base_url="https://r.jina.ai/", search_url="https://s.jina.ai/",
        max_content_length=max_content_length, timeout=timeout, search_query_content=search_query_content,
        enable_read_url="read_url" in operations, enable_search_query="search_query" in operations,
    )


def build_openrouteservice(api_key: str, timeout: int, *, session: requests.Session,
                           operations: tuple[str, ...]) -> Any:
    from agno.tools.openrouteservice import OpenRouteServiceTools
    import math

    class SelectedOpenRouteServiceTools(OpenRouteServiceTools):
        def geocode_location(self, location: str) -> dict[str, Any]:
            """Resolve one place using the donor's first-feature parser; one provider request."""
            params = {"api_key": self.api_key, "text": location, "size": 1}
            try:
                data = _service_json(session, "GET", f"{self.base_url}/pelias/v1/search",
                                     headers={}, params=params, timeout=timeout)
                result = self._parse_geocode(data, location)
                if "error" in result:
                    raise ValueError()
                for key, limit in (("longitude", 180), ("latitude", 90)):
                    number = result[key]
                    if type(number) not in (int, float) or not math.isfinite(number) or not -limit <= number <= limit:
                        raise ValueError()
                return result
            except Exception:
                pass
            raise ValueError("Service request failed")

        def get_directions(self, start_location: str, end_location: str,
                            profile: str = "driving-car") -> dict[str, Any]:
            """Resolve two locations and format one road route; at most three provider requests."""
            invalid = self._validate_profile(profile)
            if invalid:
                raise ValueError("Service request failed")
            start = self.geocode_location(start_location)
            end = self.geocode_location(end_location)
            payload = {"coordinates": [[start["longitude"], start["latitude"]], [end["longitude"], end["latitude"]]]}
            try:
                data = _service_json(session, "POST", f"{self.base_url}/openrouteservice/v2/directions/{profile}",
                                     headers=self._auth_headers(), body=payload, timeout=timeout)
                summary = data["routes"][0]["summary"]
                for field in ("distance", "duration"):
                    metric = summary[field]
                    if type(metric) not in (int, float) or not math.isfinite(metric) or metric < 0:
                        raise ValueError()
                result = self._format_directions(data, start["label"], end["label"], profile)
                if "error" in result:
                    raise ValueError()
                return result
            except Exception:
                pass
            raise ValueError("Service request failed")

    return SelectedOpenRouteServiceTools(
        api_key=api_key, base_url="https://api.heigit.org", timeout=timeout,
        enable_geocoding="geocode_location" in operations,
        enable_directions="get_directions" in operations, enable_distance_matrix=False,
    )


def build_zendesk(api_key: str, timeout: int, *, session: requests.Session,
                  subdomain: str, account_email: str | None = None,
                  auth_mode: str = "api_token") -> Any:
    from agno.tools.zendesk import ZendeskTools

    if auth_mode == "api_token" and account_email is not None:
        username, password = f"{account_email}/token", api_key
        auth = base64.b64encode(f"{username}:{password}".encode("ascii")).decode("ascii")
        authorization = f"Basic {auth}"
    elif auth_mode == "oauth_access_token" and account_email is None:
        authorization = f"Bearer {api_key}"
        # The donor constructor reads ambient credentials for falsey values.
        # These noncredential sentinels prevent those reads; only the explicit
        # authorization above is transmitted by the selected method below.
        username = password = "toolkit-oauth-unused"
    else:
        raise ValueError("Invalid Zendesk authentication mode")

    class SelectedZendeskTools(ZendeskTools):
        def search_zendesk(self, search_string: str) -> str:
            """Search the selected Zendesk Help Center and strip article HTML as the donor does."""
            url = f"https://{self.company_name}.zendesk.com/api/v2/help_center/articles/search.json?query={quote_plus(search_string)}"
            try:
                data = _service_json(session, "GET", url, headers={"Authorization": authorization}, timeout=timeout)
                if type(data["results"]) is not list:
                    raise ValueError()
                clean = re.compile("<.*?>")
                articles = [re.sub(clean, "", article["body"]) for article in data["results"]]
                return json.dumps(articles)
            except Exception:
                pass
            raise ConnectionError("Service request failed")

    return SelectedZendeskTools(username=username, password=password,
                                company_name=subdomain, timeout=timeout)


class BorrowedSessionFinancialDatasetsTools(FinancialDatasetsTools):
    """Borrow a host-configured session; this toolkit never closes it."""

    def __init__(self, api_key: str, timeout: int, *, session: requests.Session) -> None:
        if not api_key or not isinstance(session, requests.Session):
            raise ValueError("Explicit Financial Datasets configuration required")
        self._session = session
        super().__init__(api_key=api_key, timeout=timeout)

    def _make_request(self, endpoint: str, params: dict[str, Any]) -> str:
        if not self.api_key:
            return "API key not set"
        headers = {"X-API-KEY": self.api_key}
        url = f"{self.base_url}/{endpoint}"
        try:
            response = self._session.get(
                url, headers=headers, params=params, timeout=self.timeout,
                allow_redirects=False,
            )
            if 300 <= response.status_code < 400:
                return f"Error making request to {url}: redirect refused"
            response.raise_for_status()
            return response.text
        except requests.exceptions.RequestException:
            return f"Error making request to {url}: request failed"


def build_openweather(
    api_key: str, timeout: int, *, session: requests.Session, units: str,
    operations: tuple[str, ...],
) -> Any:
    """Borrow the supplied session; selected operation bodies stay upstream."""
    from agno.tools.openweather import OpenWeatherTools

    if not api_key or not isinstance(session, requests.Session):
        raise ValueError("Explicit OpenWeather configuration required")

    class BorrowedSessionOpenWeatherTools(OpenWeatherTools):
        def _make_request(self, url: str, params: dict) -> Any:
            try:
                response = session.get(
                    url, params={**params, "appid": self.api_key}, timeout=self.timeout,
                    allow_redirects=False,
                )
                if 300 <= response.status_code < 400:
                    return {"error": "OpenWeather request failed"}
                response.raise_for_status()
                return response.json()
            except (requests.exceptions.RequestException, ValueError):
                return {"error": "OpenWeather request failed"}

    return BorrowedSessionOpenWeatherTools(
        api_key=api_key, timeout=timeout, units=units,
        enable_current_weather="get_current_weather" in operations,
        enable_forecast="get_forecast" in operations,
        enable_air_pollution="get_air_pollution" in operations,
        enable_geocoding="geocode_location" in operations,
    )


def build_bitbucket(
    api_key: str, timeout: int, *, session: requests.Session, account_email: str,
    workspace: str, repo_slug: str,
) -> Any:
    """Borrow a session for bounded Bitbucket Cloud reads; never follow paging.

    Ordinary Requests/urllib3 responses support public raw.read1(), which
    returns available body bytes without filling the requested amount. A
    deadline is checked at those boundaries, including within a large HTTP
    chunk. This is not a hard deadline for DNS, connect, header or chunk-framing
    parsing, a blocking custom transport, or response cleanup.
    """
    from agno.tools.bitbucket import BitbucketTools

    if not all((api_key, account_email, workspace, repo_slug)) or not isinstance(session, requests.Session):
        raise ValueError("Explicit Bitbucket configuration required")

    class BorrowedSessionBitbucketTools(BitbucketTools):
        def _make_request(
            self, method: str, endpoint: str, params: dict[str, Any] | None = None,
            data: dict[str, Any] | None = None,
        ) -> Any:
            if method != "GET" or data is not None:
                return {"error": "Bitbucket request failed"}
            selected_params = dict(params or {})
            if endpoint == f"/repositories/{self.workspace}/{self.repo_slug}/pullrequests":
                selected_params.update(page=1, pagelen=20)
            responses = []
            try:
                deadline = time.monotonic() + self.timeout
                limit = 1_048_576

                def check_headers(response):
                    if (time.monotonic() >= deadline or not 200 <= response.status_code < 300
                            or response.headers.get("Content-Type", "application/json").split(";", 1)[0].strip().lower() != "application/json"
                            or response.headers.get("Content-Encoding", "identity").strip().lower() != "identity"):
                        raise ValueError()
                    length = response.headers.get("Content-Length")
                    if length is not None and (not re.fullmatch(r"[0-9]{1,20}", length) or int(length) > limit):
                        raise ValueError()

                def own_response(response):
                    if not isinstance(response, requests.Response):
                        raise ValueError()
                    if all(response is not held for held in responses):
                        responses.append(response)
                    return response

                def guard_response(response, *args, **kwargs):
                    # Requests can consume a redirect body while preparing
                    # response.next even with allow_redirects=False. Validate
                    # before that path, retaining every response returned by
                    # host hooks so failures and replacements also close.
                    response = own_response(response)
                    check_headers(response)
                    for hook in tuple(session.hooks.get("response", [])):
                        replacement = hook(response, *args, **kwargs)
                        if replacement is not None:
                            response = own_response(replacement)
                        check_headers(response)
                    return response

                response = own_response(session.request(
                    method, f"{self.base_url}{endpoint}", headers=self.headers,
                    params=selected_params, timeout=self.timeout, allow_redirects=False, stream=True,
                    hooks={"response": [guard_response]},
                ))
                check_headers(response)
                if response.raw is None:
                    # A caller may supply an already buffered Response. It
                    # has no network read to interrupt, but gets the same cap.
                    payload = response.content
                    if type(payload) is not bytes or len(payload) > limit:
                        raise ValueError()
                else:
                    payload = bytearray()
                    while True:
                        if time.monotonic() >= deadline:
                            raise ValueError()
                        chunk = response.raw.read1(min(16_384, limit + 1 - len(payload)), decode_content=False)
                        if time.monotonic() >= deadline:
                            raise ValueError()
                        if not chunk:
                            break
                        if type(chunk) is not bytes:
                            raise ValueError()
                        payload.extend(chunk)
                        if len(payload) > limit:
                            raise ValueError()
                if time.monotonic() >= deadline:
                    raise ValueError()
                result = json.loads(payload.decode("utf-8"), parse_constant=lambda _: _invalid_json()) if payload else {}
            except BaseException as error:
                if isinstance(error, requests.RequestException) and isinstance(error.response, requests.Response):
                    own_response(error.response)
                if not isinstance(error, Exception):
                    raise
                result = {"error": "Bitbucket request failed"}
            finally:
                for response in reversed(responses):
                    try:
                        response.close()
                    except Exception:
                        result = {"error": "Bitbucket request failed"}
            return result

    # All three fields are explicit, so none of the donor's independent
    # username/password/token environment fallbacks is evaluated.
    toolkit = BorrowedSessionBitbucketTools(
        username=account_email, password=api_key, token=api_key,
        workspace=workspace, repo_slug=repo_slug, timeout=timeout,
        server_url="api.bitbucket.org", api_version="2.0",
    )
    # Request identity so wire and decoded caps coincide. A decoder can wait
    # for more compressed input internally and hide the body read boundaries.
    toolkit.headers["Accept-Encoding"] = "identity"
    return toolkit


def build_gitlab(
    api_key: str, timeout: int, *, session: requests.Session, host: str,
    operations: tuple[str, ...],
) -> Any:
    """Borrow one session through python-gitlab's actual sync SDK instance."""
    from agno.tools.gitlab import GitlabTools
    from gitlab import Gitlab
    from gitlab.exceptions import GitlabError

    if not api_key or not host or not isinstance(session, requests.Session):
        raise ValueError("Explicit GitLab configuration required")

    class NoRetryGitlab(Gitlab):
        def http_request(self, verb: str, path: str, **kwargs: Any) -> requests.Response:
            kwargs.update(obey_rate_limit=False, max_retries=0, retry_transient_errors=False)
            try:
                return super().http_request(verb, path, **kwargs)
            except Exception:
                pass
            # Raise outside the handler: the inherited donor logs exceptions,
            # so do not retain a provider diagnostic as chained context.
            raise GitlabError("GitLab request failed")

    class BorrowedSessionGitlabTools(GitlabTools):
        def _create_client(self) -> Any:
            return NoRetryGitlab(
                url=self.base_url, private_token=self.access_token, timeout=self.timeout,
                session=session, ssl_verify=True, keep_base_url=True,
            )

    return BorrowedSessionGitlabTools(
        access_token=api_key, base_url=host, timeout=timeout,
        enable_list_projects="list_projects" in operations,
        enable_get_projects="get_project" in operations,
        enable_list_merge_requests="list_merge_requests" in operations,
        enable_get_merge_request="get_merge_request" in operations,
        enable_list_issues="list_issues" in operations,
    )


# Additional sources at the Agno commit named above:
# brightdata.py: b9a0fe8b6558fa8fb186b63d34f1c11186afe7df8822da420f332a5e9044fd83
# linkup.py: 5e276d832f9ca8f9a5d469f22cd445a1a815aa8420c7409da2e5dbdefceaac77
# todoist.py: 62560033c0ef1a8db07b01e4946a12daec2ac01ce101f5cfbf50d7fb05711325
# zoom.py: b931845fd52a5d6fd7abc8275e9f51d1a67f11a17c252586873df92a45573e98
# Bright Data and Linkup inherit their actual operation bodies. Successful text
# and SDK models are projected into plain JSON, preserving the distinction
# between provider content and donor error strings. Todoist ports construction
# to inject a real SDK client while inheriting get_task and _task_to_dict.
# Zoom explicitly ports two read bodies and construction for a selected bearer
# token, without importing the donor's OAuth acquisition/refresh behavior.


def build_brightdata(api_key: str, timeout: int, *, session: requests.Session,
                     web_unlocker_zone: str, serp_zone: str, operations: tuple[str, ...]) -> Any:
    from agno.tools.brightdata import BrightDataTools

    class BorrowedBrightDataTools(BrightDataTools):
        def _make_request(self, payload: dict) -> Any:
            try:
                if (set(payload) != {"url", "zone", "format", "data_format"}
                        or payload["zone"] not in (web_unlocker_zone, serp_zone)
                        or payload["format"] != "raw" or payload["data_format"] != "markdown"):
                    raise ValueError()
                target = payload["url"]
                parsed = urlsplit(target)
                if (parsed.scheme not in ("http", "https") or not parsed.hostname
                        or parsed.username is not None or parsed.password is not None
                        or "\\" in target or any(ord(c) <= 32 or 0xD800 <= ord(c) <= 0xDFFF for c in target)
                        or (parsed.port is not None and not 1 <= parsed.port <= 65535)):
                    raise ValueError()
                raw = _service_bytes(session, "POST", "https://api.brightdata.com/request",
                                     timeout=timeout, headers={"Authorization": f"Bearer {api_key}"},
                                     body=payload, expected_status=200)
                # A scraped page may start with the same text as a donor error.
                # Keep successful provider text in data, never in that channel.
                return {"content": raw.decode("utf-8")}
            except Exception:
                pass
            raise ValueError("Service request failed")

    toolkit = BorrowedBrightDataTools(
        api_key=api_key, timeout=timeout, web_unlocker_zone=web_unlocker_zone, serp_zone=serp_zone,
        enable_scrape_markdown="scrape_as_markdown" in operations,
        enable_search_engine="search_engine" in operations,
        enable_screenshot=False, enable_web_data_feed=False,
    )
    # The donor lets ambient zone variables override explicit arguments.
    toolkit.web_unlocker_zone = web_unlocker_zone
    toolkit.serp_zone = serp_zone
    return toolkit


def build_linkup(api_key: str, timeout: int, *, session: requests.Session,
                 depth: str, output_type: str) -> Any:
    from agno.tools.linkup import LinkupTools
    from linkup import LinkupClient
    import httpx

    class BorrowedLinkupClient(LinkupClient):
        def _request(self, method, url, *, json=None, params=None, timeout=None):
            if (method != "POST" or url != "/search" or params is not None
                    or type(json) is not dict or set(json) != {"q", "depth", "outputType"}
                    or json["depth"] not in ("standard", "deep")
                    or json["outputType"] not in ("searchResults", "sourcedAnswer")):
                raise ValueError("Service request failed")
            value = _service_json(session, "POST", "https://api.linkup.so/v1/search",
                                  timeout=request_timeout, headers={"Authorization": f"Bearer {api_key}"},
                                  body=json, expected_status=200)
            return httpx.Response(200, json=value)

        def search(self, *args, **kwargs):
            try:
                # Real SDK parameter construction and Pydantic response parsing.
                return super().search(*args, **kwargs).model_dump(mode="json")
            except Exception:
                pass
            raise ValueError("Service request failed")

    request_timeout = timeout
    toolkit = LinkupTools(api_key=api_key, depth=depth, output_type=output_type)
    # Linkup's constructor stores configuration only, opening no client/session.
    toolkit.linkup = BorrowedLinkupClient(api_key=api_key, base_url="https://api.linkup.so/v1")
    return toolkit


def _todoist_task_shape(value: Any, task_id: str) -> None:
    """Reject lossy scalar coercion before the real SDK constructs its Task."""
    if type(value) is not dict or value.get("id") != task_id:
        raise ValueError("Service request failed")
    for key in ("id", "content", "description", "project_id"):
        if type(value.get(key)) is not str:
            raise ValueError("Service request failed")
    for keys in (("added_by_uid", "creator_id"), ("added_at", "created_at"), ("updated_at",)):
        for key in keys:
            if key in value:
                if type(value[key]) is not str or not value[key]:
                    raise ValueError("Service request failed")
                break
        else:
            raise ValueError("Service request failed")
    for key in ("section_id", "parent_id"):
        if value.get(key) is not None and type(value[key]) is not str:
            raise ValueError("Service request failed")
    order = value.get("child_order", value.get("order"))
    if type(order) is not int or type(value.get("priority")) is not int or not 1 <= value["priority"] <= 4:
        raise ValueError("Service request failed")
    labels = value.get("labels")
    if labels is not None and (type(labels) is not list or any(type(label) is not str for label in labels)):
        raise ValueError("Service request failed")
    due = value.get("due")
    if due is not None:
        if (type(due) is not dict or type(due.get("date")) is not str
                or type(due.get("string")) is not str
                or ("is_recurring" in due and type(due["is_recurring"]) is not bool)
                or ("lang" in due and type(due["lang"]) is not str)
                or (due.get("timezone") is not None and type(due["timezone"]) is not str)):
            raise ValueError("Service request failed")


def build_todoist(api_key: str, timeout: int, *, session: requests.Session, resources: Any) -> Any:
    from agno.tools import Toolkit
    from agno.tools.todoist import TodoistTools
    from todoist_api_python.api import TodoistAPI
    import httpx

    class BorrowedTodoistTransport(httpx.BaseTransport):
        def handle_request(self, request):
            if (request.method != "GET" or request.url.scheme != "https"
                    or request.url.host != "api.todoist.com" or request.url.port not in (None, 443)
                    or request.url.query or request.url.fragment or request.url.userinfo
                    or not re.fullmatch(r"/api/v1/tasks/[A-Za-z0-9_-]{1,128}", request.url.path)
                    or request.headers.get("authorization") != f"Bearer {api_key}"):
                raise ValueError("Service request failed")
            value = _service_json(session, "GET", str(request.url), timeout=timeout,
                                  headers={"Authorization": f"Bearer {api_key}"}, expected_status=200)
            _todoist_task_shape(value, request.url.path.rsplit("/", 1)[1])
            # Actual SDK HTTPX protocol boundary, after the sole I/O owner has
            # fully consumed and closed the provider response. No second socket.
            return httpx.Response(200, json=value)

    class SelectedTodoistAPI(TodoistAPI):
        def get_task(self, task_id):
            try:
                return super().get_task(task_id)
            except Exception:
                pass
            # Dataclass parsing errors can otherwise include provider values
            # in the donor's logger.exception call and JSON error result.
            raise ValueError("Service request failed")

    class SelectedTodoistTools(TodoistTools):
        def __init__(self):
            self.api_token = api_key
            client = httpx.Client(transport=BorrowedTodoistTransport(), trust_env=False, follow_redirects=False)
            try:
                self.api = SelectedTodoistAPI(api_key, client=client, request_id_fn=None)
            except Exception:
                client.close()
                raise
            resources.callback(self.api.__exit__, None, None, None)
            Toolkit.__init__(self, name="todoist", tools=[self.get_task])

    return SelectedTodoistTools()


def build_zoom(api_key: str, timeout: int, *, session: requests.Session, operations: tuple[str, ...]) -> Any:
    from agno.tools import Toolkit
    from agno.tools.zoom import ZoomTools

    class SelectedZoomTools(ZoomTools):
        def __init__(self):
            Toolkit.__init__(self, name="zoom_tool", tools=[getattr(self, op) for op in operations], timeout=timeout)

        def list_meetings(self, user_id: str = "me", type: str = "scheduled") -> str:
            """Read the first meeting page for a Zoom user ID or 'me', without following pagination."""
            try:
                if not re.fullmatch(r"[A-Za-z0-9_-]{1,128}", user_id) or type not in ("scheduled", "live", "upcoming", "previous"):
                    raise ValueError()
                meetings = _service_json(session, "GET", f"https://api.zoom.us/v2/users/{user_id}/meetings",
                                         timeout=timeout, headers={"Authorization": f"Bearer {api_key}"},
                                         params={"type": type}, expected_status=200)
                # Use __class__ here because the upstream parameter is 'type'.
                if (meetings.__class__ is not dict or meetings.get("meetings").__class__ is not list
                        or any(item.__class__ is not dict or item.get("id").__class__ not in (int, str)
                               or not re.fullmatch(r"[0-9]{1,20}", str(item["id"]))
                               or item.get("topic").__class__ is not str for item in meetings["meetings"])):
                    raise ValueError()
                for key in ("page_count", "page_number", "page_size", "total_records"):
                    if key in meetings and (meetings[key].__class__ is not int or meetings[key] < 0):
                        raise ValueError()
                result = {"message": "Meetings retrieved successfully", "meetings": meetings["meetings"]}
                for key in ("page_count", "page_number", "page_size", "total_records"):
                    if key in meetings:
                        result[key] = meetings[key]
                return json.dumps(result, indent=2)
            except Exception:
                return json.dumps({"error": "Service request failed"})

        def get_meeting(self, meeting_id: str) -> str:
            """Read one numeric Zoom meeting ID using the explicitly selected bearer token."""
            try:
                if not re.fullmatch(r"[0-9]{1,20}", meeting_id):
                    raise ValueError()
                value = _service_json(session, "GET", f"https://api.zoom.us/v2/meetings/{meeting_id}",
                                      timeout=timeout, headers={"Authorization": f"Bearer {api_key}"}, expected_status=200)
                if (type(value) is not dict or type(value.get("id")) not in (str, int)
                        or str(value["id"]) != meeting_id or type(value.get("topic")) is not str):
                    raise ValueError()
                if "duration" in value and (type(value["duration"]) is not int or value["duration"] < 0):
                    raise ValueError()
                if "settings" in value and type(value["settings"]) is not dict:
                    raise ValueError()
                if "type" in value and type(value["type"]) is not int:
                    raise ValueError()
                for key in ("start_time", "timezone", "created_at", "join_url"):
                    if key in value and type(value[key]) is not str:
                        raise ValueError()
                result = {"message": "Meeting details retrieved successfully",
                          "meeting_id": str(value["id"]), "topic": value["topic"]}
                # An absent provider field is unknown, not an empty/zero fact.
                for key in ("type", "start_time", "duration", "timezone", "created_at", "join_url", "settings"):
                    if key in value:
                        result[key] = value[key]
                return json.dumps(result, indent=2)
            except Exception:
                return json.dumps({"error": "Service request failed"})

    return SelectedZoomTools()


class _SdkTransport:
    """Borrow one owned session; return real, fully buffered Requests responses.

    SDK operation bodies and response models run normally. This seam owns only
    URL confinement, selected credentials, bounded I/O and provider shape checks.
    The provider status must really be 200; no synthetic success fallback exists.
    """

    def __init__(self, session, timeout, origin, headers, route, shape, *, borrowed_auth=False):
        self._session, self._timeout = session, timeout
        self._origin, self._headers = origin, headers
        self._route, self._shape = route, shape
        self._borrowed_auth = borrowed_auth
        self.proxies = {}

    def request(self, method, url, **kwargs):
        try:
            parsed = urlsplit(url)
            if (f"{parsed.scheme}://{parsed.netloc}" != self._origin or parsed.fragment
                    or parsed.username is not None or parsed.password is not None
                    or "\\" in url or any(ord(c) <= 32 for c in url)):
                raise ValueError()
            params = dict(kwargs.get("params") or {})
            body = kwargs.get("json")
            if kwargs.get("data") not in (None, {}, "", b"") or kwargs.get("files"):
                raise ValueError()
            self._route(method, parsed, params, body)
            if self._borrowed_auth:
                _require_salesforce_client(self._session, self._origin)
            response_headers = {}
            read = _borrowed_salesforce_bytes if self._borrowed_auth else _service_bytes
            payload = read(self._session, method, url, timeout=self._timeout,
                                     headers=self._headers, params=params, body=body,
                                     expected_status=200, response_headers=response_headers)
            self._shape(payload, parsed, response_headers)
            response = requests.Response()
            response.status_code = 200
            response.url, response._content = url, payload
            response._content_consumed = True
            response.headers.update(response_headers)
            response.encoding = "utf-8"
            return response
        except Exception:
            pass
        raise ValueError("Service request failed")

    def get(self, url, **kwargs):
        return self.request("GET", url, **kwargs)

    def close(self):
        # The caller that supplied the session owns its close after workers join.
        pass


def _sdk_json(payload):
    value = json.loads(payload.decode("utf-8"), parse_constant=lambda _: _invalid_json())
    if type(value) not in (dict, list):
        raise ValueError()
    return value


def _sdk_strings(value, required, optional=()):
    if type(value) is not dict:
        raise ValueError()
    for key in required:
        if type(value.get(key)) is not str:
            raise ValueError()
    for key in optional:
        if value.get(key) is not None and type(value[key]) is not str:
            raise ValueError()


def build_exa(api_key: str, timeout: int, *, session: requests.Session) -> Any:
    from agno.tools.exa import ExaTools
    from exa_py import Exa

    def route(method, url, params, body):
        if method != "POST" or url.path != "/search" or url.query or params or type(body) is not dict:
            raise ValueError()

    def shape(payload, url, headers):
        value = _sdk_json(payload)
        if type(value) is not dict or type(value.get("results")) is not list or len(value["results"]) > 20:
            raise ValueError()
        for item in value["results"]:
            _sdk_strings(item, ("url", "id"), ("title", "author", "publishedDate", "text"))

    transport = _SdkTransport(session, timeout, "https://api.exa.ai", {"x-api-key": api_key}, route, shape)

    class SelectedExa(Exa):
        def request(self, endpoint, data=None, method="POST", params=None, headers=None):
            return transport.request(method, self.base_url + endpoint, params=params, json=data).json()

        def search_and_contents(self, *args, **kwargs):
            try:
                return super().search_and_contents(*args, **kwargs)
            except Exception:
                pass
            # The donor logs exceptions; SDK parser diagnostics must stay local.
            raise ValueError("Service request failed")

    toolkit = ExaTools(api_key=api_key, timeout=timeout, enable_get_contents=False,
                       enable_find_similar=False, enable_answer=False, text_length_limit=10_000)
    toolkit.exa = SelectedExa(api_key=api_key, base_url="https://api.exa.ai")
    return toolkit


def build_firecrawl(api_key: str, timeout: int, *, session: requests.Session, operations: tuple[str, ...]) -> Any:
    from agno.tools.firecrawl import FirecrawlTools
    from firecrawl.v2.utils.http_client import HttpClient

    def route(method, url, params, body):
        if (method != "POST" or url.path not in ("/v2/scrape", "/v2/map")
                or url.query or params or type(body) is not dict):
            raise ValueError()
        target = body.get("url")
        parsed = urlsplit(target)
        if (parsed.scheme not in ("http", "https") or not parsed.hostname or parsed.username is not None
                or parsed.password is not None or "\\" in target or any(ord(c) <= 32 for c in target)):
            raise ValueError()

    def shape(payload, url, headers):
        value = _sdk_json(payload)
        if type(value) is not dict or value.get("success") is not True:
            raise ValueError()
        if url.path == "/v2/scrape":
            if type(value.get("data")) is not dict or type(value["data"].get("markdown")) is not str:
                raise ValueError()
        else:
            links = value.get("links")
            if type(links) is not list or len(links) > 100:
                raise ValueError()
            for link in links:
                _sdk_strings(link, ("url",), ("title", "description"))

    transport = _SdkTransport(session, timeout, "https://api.firecrawl.dev",
                              {"Authorization": f"Bearer {api_key}"}, route, shape)

    class SelectedHttpClient(HttpClient):
        def post(self, endpoint, data, **kwargs):
            # The real SDK prepared this body; retain its attribution field.
            payload = {**data, "origin": data.get("origin") or self.origin}
            return transport.request("POST", self._build_url(endpoint), json=payload)

    toolkit = FirecrawlTools(api_key=api_key, api_url="https://api.firecrawl.dev", formats=["markdown"],
                             enable_scrape="scrape_website" in operations,
                             enable_mapping="map_website" in operations)
    toolkit.app._v2_client.http_client = SelectedHttpClient(api_key, "https://api.firecrawl.dev", timeout=timeout, max_retries=0)
    return toolkit


def build_trello(api_key: str, timeout: int, *, session: requests.Session, token: str) -> Any:
    from agno.tools import Toolkit
    from agno.tools.trello import TrelloTools
    from trello import TrelloClient
    from trello.board import Board

    def route(method, url, params, body):
        query = parse_qs(url.query, strict_parsing=True)
        if (method != "GET" or url.path != "/1/members/me/boards/" or body is not None
                or set(query) != {"filter"} or len(query["filter"]) != 1
                or query["filter"][0] not in ("all", "open", "closed", "organization", "public", "starred")
                or params != {"key": api_key, "token": token}):
            raise ValueError()

    def shape(payload, url, headers):
        value = _sdk_json(payload)
        if type(value) is not list or len(value) > 100:
            raise ValueError()
        for board in value:
            _sdk_strings(board, ("id", "name", "url"), ("desc", "idOrganization"))
            if type(board.get("closed")) is not bool or (board.get("starred") is not None and type(board["starred"]) is not bool):
                raise ValueError()

    transport = _SdkTransport(session, timeout, "https://api.trello.com", {}, route, shape)

    class SelectedTrelloTools(TrelloTools):
        def __init__(self):
            # Explicit constructor port. py-trello's non-OAuth mode stores the
            # user token in api_secret and emits the real key/token query pair.
            self.client = TrelloClient(api_key=api_key, api_secret=token, http_service=transport)
            Toolkit.__init__(self, name="trello", tools=[self.list_boards])

        def list_boards(self, board_filter: str = "all") -> str:
            """Get the authenticated user's boards with provider metadata preserved.

            Args:
                board_filter: Filter for boards; defaults to all.
            """
            # Narrow operation port of the SDK list_boards orchestration and
            # donor projection. Real fetch_json and Board.from_json run, but
            # optional fields dropped/defaulted by Board retain provider
            # presence and values rather than invented false/null defaults.
            values = self.client.fetch_json("/members/me/boards/?filter=%s" % board_filter)
            boards = []
            for value in values:
                board = Board.from_json(self.client, json_obj=value)
                item = {"id": board.id, "name": board.name, "url": board.url, "closed": board.closed}
                for source, target in (("desc", "description"), ("starred", "starred"), ("idOrganization", "organization")):
                    if source in value:
                        item[target] = value[source]
                boards.append(item)
            return json.dumps({"filter_used": board_filter, "total_boards": len(boards), "boards": boards})

    return SelectedTrelloTools()


def build_salesforce(api_key: str, timeout: int, *, session: requests.Session, instance_url: str) -> Any:
    return _build_salesforce(api_key, timeout, session=session, instance_url=instance_url)


def build_salesforce_from_client(timeout: int, *, session: ServiceSession, instance_url: str) -> Any:
    """Borrow prepared host auth; no token snapshot or environment fallback."""
    _require_salesforce_client(session, instance_url)
    return _build_salesforce(None, timeout, session=session, instance_url=instance_url)


def _build_salesforce(api_key: str | None, timeout: int, *, session: requests.Session, instance_url: str) -> Any:
    from agno.tools import Toolkit
    from agno.tools.salesforce import SalesforceTools
    from simple_salesforce import Salesforce

    def route(method, url, params, body):
        if (method != "GET" or url.query or params or body is not None
                or not re.fullmatch(r"/services/data/v[0-9]{1,3}\.0/sobjects/[A-Z][A-Za-z0-9_]{0,79}/[A-Za-z0-9]{15}(?:[A-Za-z0-9]{3})?", url.path)):
            raise ValueError()

    def shape(payload, url, headers):
        value = _sdk_json(payload)
        selected_id = url.path.rsplit("/", 1)[1]
        record_id = value.get("Id") if type(value) is dict else None
        if (type(record_id) is not str or not re.fullmatch(r"[A-Za-z0-9]{15}(?:[A-Za-z0-9]{3})?", record_id)
                or not (record_id == selected_id or (len(selected_id) == 15 and len(record_id) == 18 and record_id[:15] == selected_id))):
            raise ValueError()

    transport = _SdkTransport(session, timeout, instance_url,
        {} if api_key is None else {"Authorization": f"Bearer {api_key}"}, route, shape,
        borrowed_auth=api_key is None)

    class SelectedSalesforceTools(SalesforceTools):
        def __init__(self):
            # Session-token constructor port: no password login or instructions
            # are imported into the toolkit's agent/runtime owner.
            # An explicit empty constructor value selects the SDK's direct
            # session mode for borrowed auth. Its generated header is never
            # forwarded: only the host Session.auth prepares that request.
            self.sf = Salesforce(instance_url=instance_url,
                session_id="" if api_key is None else api_key, session=transport)
            Toolkit.__init__(self, name="salesforce_tools", tools=[self.get_record])

    return SelectedSalesforceTools()


def build_twilio(api_key: str, timeout: int, *, session: requests.Session, account_sid: str) -> Any:
    from datetime import datetime, timezone
    import logging
    from agno.tools import Toolkit
    from agno.tools.twilio import TwilioTools
    from twilio.base.deserialize import rfc2822_datetime
    from twilio.http import HttpClient
    from twilio.http.response import Response
    from twilio.rest import Client

    def route(method, url, params, body):
        if (method != "GET" or url.query or params or body is not None
                or not re.fullmatch(r"/2010-04-01/Accounts/" + re.escape(account_sid) + r"/Calls/CA[0-9a-fA-F]{32}\.json", url.path)):
            raise ValueError()

    def shape(payload, url, headers):
        value = _sdk_json(payload)
        _sdk_strings(value, ("sid", "account_sid", "to", "from", "status"),
                     ("duration", "direction", "price", "start_time", "end_time"))
        if value["account_sid"] != account_sid or value["sid"] != url.path.rsplit("/", 1)[1][:-5]:
            raise ValueError()
        for key in ("start_time", "end_time"):
            if value.get(key) is not None:
                # Accept a complete supported UTC wire form, not the date
                # prefix accepted by permissive RFC parsers. No whitespace or
                # comment stripping; the actual SDK model still parses it.
                match = re.fullmatch(
                    r"(Mon|Tue|Wed|Thu|Fri|Sat|Sun), ([0-9]{2}) "
                    r"(Jan|Feb|Mar|Apr|May|Jun|Jul|Aug|Sep|Oct|Nov|Dec) "
                    r"([0-9]{4}) ([0-9]{2}):([0-9]{2}):([0-9]{2}) (?:\+0000|GMT)",
                    value[key],
                )
                if match is None:
                    raise ValueError()
                weekday, day, month, year, hour, minute, second = match.groups()
                months = ("Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec")
                timestamp = datetime(int(year), months.index(month) + 1, int(day),
                                     int(hour), int(minute), int(second), tzinfo=timezone.utc)
                if (("Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun")[timestamp.weekday()] != weekday
                        or rfc2822_datetime(value[key]) != timestamp):
                    # The pinned SDK reinterprets years below 0100. Its parser
                    # must preserve the exact validated instant as well.
                    raise ValueError()

    credential = base64.b64encode(f"{account_sid}:{api_key}".encode()).decode()
    transport = _SdkTransport(session, timeout, "https://api.twilio.com", {"Authorization": "Basic " + credential}, route, shape)

    class SelectedHttpClient(HttpClient):
        def request(self, method, uri, params=None, data=None, headers=None, auth=None, timeout=None, allow_redirects=False):
            response = transport.request(method, uri, params=params, data=data)
            return Response(response.status_code, response.text, response.headers)

    class SelectedTwilioTools(TwilioTools):
        def __init__(self):
            # Avoid ambient alternative API keys, edge or region selection.
            self.client = Client(account_sid, api_key, account_sid,
                                 http_client=SelectedHttpClient(logging.getLogger(__name__), False, timeout))
            self.client.region, self.client.edge = None, None
            Toolkit.__init__(self, name="twilio", tools=[self.get_call_details])

        def get_call_details(self, call_sid: str) -> dict[str, Any]:
            """Get details about a specific call.

            Args:
                call_sid: The SID of the call to lookup.
            """
            # Operation port: the donor's from_ lookup is incompatible with
            # Twilio 9.11.2. Its real CallInstance field is _from. Transport
            # failures raise before the SDK can expose provider diagnostics.
            call = self.client.calls(call_sid).fetch()
            return {"to": call.to, "from": call._from, "status": call.status,
                    "duration": call.duration, "direction": call.direction,
                    "price": call.price, "start_time": str(call.start_time), "end_time": str(call.end_time)}

    return SelectedTwilioTools()


def build_webex(api_key: str, timeout: int, *, session: requests.Session) -> Any:
    from agno.tools import Toolkit
    from agno.tools.webex import WebexTools
    from webexpythonsdk import WebexAPI

    def route(method, url, params, body):
        if method != "GET" or url.path != "/v1/rooms" or url.query or params != {"max": 100} or body is not None:
            raise ValueError()

    def shape(payload, url, headers):
        value = _sdk_json(payload)
        if (type(value) is not dict or type(value.get("items")) is not list or len(value["items"]) > 100
                or any(key.lower() == "link" and bool(value) for key, value in headers.items())):
            # The donor claims all rooms. Pagination is an explicit failure,
            # never silent truncation and never an untrusted next-URL fetch.
            raise ValueError()
        for room in value["items"]:
            _sdk_strings(room, ("id", "title", "type"))
            for key in ("isPublic", "isReadOnly"):
                if key in room and type(room[key]) is not bool:
                    raise ValueError()

    transport = _SdkTransport(session, timeout, "https://webexapis.com", {"Authorization": f"Bearer {api_key}"}, route, shape)

    class SelectedWebexTools(WebexTools):
        def __init__(self):
            self.client = WebexAPI(access_token=api_key, base_url="https://webexapis.com/v1/",
                                   single_request_timeout=timeout, wait_on_rate_limit=False,
                                   be_geo_id="meerkat-toolkit", caller="meerkat-toolkit")
            self.client._session._req_session.close()
            self.client._session._req_session = transport
            Toolkit.__init__(self, name="webex", tools=[self.list_rooms])

    return SelectedWebexTools()


def build_plivo(api_key: str, timeout: int, *, session: requests.Session, auth_id: str, resources: Any) -> Any:
    from agno.tools import Toolkit
    from agno.tools.plivo import PlivoTools
    from plivo import RestClient

    def route(method, url, params, body):
        query = parse_qs(url.query, strict_parsing=True)
        if (method != "GET" or query != {"call_uuid": [url.path.rstrip("/").rsplit("/", 1)[1]]} or params or body is not None
                or not re.fullmatch(r"/v1/Account/" + re.escape(auth_id) + r"/Call/[0-9a-fA-F-]{36}/", url.path)):
            raise ValueError()

    def shape(payload, url, headers):
        value = _sdk_json(payload)
        _sdk_strings(value, ("call_uuid", "to_number", "from_number", "call_state"),
                     ("call_direction", "initiation_time", "answer_time", "total_amount"))
        if value["call_uuid"] != url.path.rstrip("/").rsplit("/", 1)[1]:
            raise ValueError()
        duration = value.get("call_duration")
        if duration is not None and (type(duration) is not int or duration < 0):
            raise ValueError()

    credential = base64.b64encode(f"{auth_id}:{api_key}".encode()).decode()
    transport = _SdkTransport(session, timeout, "https://api.plivo.com", {"Authorization": "Basic " + credential}, route, shape)

    class SelectedClient(RestClient):
        def send_request(self, request, **kwargs):
            # Use the real SDK request preparation and resource models. A
            # non-200 response raises here before SDK region retries/fallback.
            return transport.request(request.method, request.url)

    class SelectedPlivoTools(PlivoTools):
        def __init__(self):
            self.client = SelectedClient(auth_id=auth_id, auth_token=api_key, timeout=timeout)
            resources.callback(self.client.session.close)
            resources.callback(self.client.multipart_session.close)
            Toolkit.__init__(self, name="plivo", tools=[self.get_call_details])

    return SelectedPlivoTools()


def build_arxiv(timeout: int, *, session: requests.Session) -> Any:
    from agno.tools import Toolkit
    from agno.tools.arxiv import ArxivTools
    import arxiv
    from arxiv import _feed
    from lxml import etree

    def route(method, url, params, body):
        query = parse_qs(url.query, strict_parsing=True)
        if (method != "GET" or url.path != "/api/query" or params or body is not None
                or set(query) - {"search_query", "id_list", "start", "max_results", "sortBy", "sortOrder"}
                or any(len(values) != 1 for values in query.values())
                or query.get("start") != ["0"] or not 1 <= int(query["max_results"][0]) <= 20):
            raise ValueError()

    def shape(payload, url, headers):
        parser = etree.XMLParser(resolve_entities=False, no_network=True, huge_tree=False, recover=False)
        root = etree.fromstring(payload, parser=parser)
        ns = {"a": "http://www.w3.org/2005/Atom", "o": "http://a9.com/-/spec/opensearch/1.1/"}
        if root.tag != "{http://www.w3.org/2005/Atom}feed" or root.getroottree().docinfo.doctype:
            raise ValueError()
        counts = [root.findtext("o:" + key, namespaces=ns) for key in ("totalResults", "itemsPerPage", "startIndex")]
        if any(value is None or not re.fullmatch(r"[0-9]+", value) for value in counts):
            raise ValueError()
        total, page_size, start = map(int, counts)
        entries = root.findall("a:entry", ns)
        parsed = _feed.parse(payload)
        if (parsed.malformed or len(parsed.results) != len(entries) or len(entries) > 20 or start != 0
                or total < len(entries) or page_size < len(entries) or (not entries and total != 0)):
            raise ValueError()
        for result in parsed.results:
            if not result.title or not result.authors or not result.primary_category or not result.summary:
                raise ValueError()

    transport = _SdkTransport(session, timeout, "https://export.arxiv.org", {}, route, shape)

    class SelectedArxivTools(ArxivTools):
        def __init__(self):
            # Constructor port selects the real SDK/parser without setting up
            # the donor's PDF download path or an unbounded retrying client.
            self.client = arxiv.Client(page_size=20, delay_seconds=0, num_retries=0)
            self.client._session.close()
            self.client._session = transport
            Toolkit.__init__(self, name="arxiv_tools", tools=[self.search_arxiv_and_return_articles])

    return SelectedArxivTools()


# Agno 3.0.11 HTTP read ports. Constructor/transport deltas and the two
# source-operation ports are attributed in THIRD_PARTY_NOTICES.md.
def _sdk_numbers(value, required, *, integer=False, nonnegative=False):
    for key in required:
        item = value.get(key)
        if type(item) not in ((int,) if integer else (int, float)) or not math.isfinite(item):
            raise ValueError()
        if nonnegative and item < 0:
            raise ValueError()


def build_reddit(api_key: str, timeout: int, *, session: requests.Session, user_agent: str) -> Any:
    import configparser
    from importlib.resources import files
    import praw
    from praw.config import Config
    from agno.tools.reddit import RedditTools

    def route(method, url, params, body):
        if method != "GET" or body is not None or url.query:
            raise ValueError()
        if re.fullmatch(r"/r/[A-Za-z0-9_-]{1,32}/top", url.path):
            if (set(params) != {"t", "limit", "raw_json"} or type(params["limit"]) is not int
                    or not 1 <= params["limit"] <= 25
                    or params["t"] not in ("hour", "day", "week", "month", "year", "all")):
                raise ValueError()
        elif not re.fullmatch(r"/(?:r|user)/[A-Za-z0-9_-]{1,32}/about/", url.path) or params != {"raw_json": 1}:
            raise ValueError()

    def shape(payload, url, headers):
        value = _sdk_json(payload)
        if type(value) is not dict or "error" in value or type(value.get("data")) is not dict:
            raise ValueError()
        data = value["data"]
        if url.path.endswith("/top"):
            children = data.get("children")
            if value.get("kind") != "Listing" or type(children) is not list or len(children) > 25:
                raise ValueError()
            for child in children:
                if type(child) is not dict or child.get("kind") != "t3":
                    raise ValueError()
                post = child.get("data")
                _sdk_strings(post, ("id", "title", "url", "selftext", "permalink", "subreddit", "subreddit_name_prefixed"))
                if type(post.get("author")) is not str or post["subreddit"].lower() != url.path.split("/")[2].lower():
                    raise ValueError()
                _sdk_numbers(post, ("score",), integer=True)
                _sdk_numbers(post, ("created_utc",), nonnegative=True)
        elif url.path.startswith("/r/"):
            if value.get("kind") != "t5":
                raise ValueError()
            _sdk_strings(data, ("display_name", "description", "public_description"))
            if data["display_name"].lower() != url.path.split("/")[2].lower():
                raise ValueError()
            _sdk_numbers(data, ("subscribers", "active_user_count"), integer=True, nonnegative=True)
            _sdk_numbers(data, ("created_utc",), nonnegative=True)
            if type(data.get("over18")) is not bool:
                raise ValueError()
        else:
            if value.get("kind") != "t2":
                raise ValueError()
            _sdk_strings(data, ("name",))
            if data["name"].lower() != url.path.split("/")[2].lower():
                raise ValueError()
            _sdk_numbers(data, ("comment_karma", "link_karma"), integer=True)
            _sdk_numbers(data, ("created_utc",), nonnegative=True)
            if any(type(data.get(key)) is not bool for key in ("is_mod", "is_gold", "is_employee")):
                raise ValueError()

    bridge = _SdkTransport(session, timeout, "https://oauth.reddit.com",
                           {"Authorization": f"bearer {api_key}", "User-Agent": user_agent}, route, shape)

    class SelectedReddit(praw.Reddit):
        def __init__(self):
            # Port only construction: read the installed SDK's own defaults,
            # never Config.CONFIG, home/cwd configuration or environment keys.
            parser = configparser.ConfigParser(interpolation=None)
            parser.read_string(files("praw").joinpath("praw.ini").read_text())
            config = Config.__new__(Config)
            selected = {key: None for key in ("client_id", "client_secret", "redirect_uri", "refresh_token", "password", "username")}
            selected.update(user_agent=user_agent, check_for_updates=False, check_for_async=False, timeout=timeout)
            config._settings = selected
            config.custom = {**parser.defaults(), **selected}
            config._initialize_attributes()
            self.config, self._unique_counter = config, 0
            self._core = self._authorized_core = self._read_only_core = None
            self._prepare_objector()
            self.subreddit = praw.models.SubredditHelper(self, None)

        def request(self, *, method, path, params=None, data=None, files=None, json=None):
            # PRAW's real models, listing and objectification remain intact.
            # This replaces credential discovery, retries and rate-limit sleep.
            if data is not None or files is not None or json is not None:
                raise ValueError("Service request failed")
            query = {**(params or {}), "raw_json": 1}
            value = bridge.request(method, "https://oauth.reddit.com/" + path.lstrip("/"), params=query).json()
            if path.endswith("/top"):
                children = value["data"]["children"]
                if len(children) > query["limit"] or (value["data"].get("after") and len(children) < query["limit"]):
                    raise ValueError("Service request failed")
            return value

    return RedditTools(reddit_instance=SelectedReddit())


def build_x(api_key: str, timeout: int, *, session: requests.Session, include_post_metrics: bool) -> Any:
    import tweepy
    from agno.tools import Toolkit
    from agno.tools.x import XTools

    def canonical_id(value):
        # Tweepy normalizes IDs to integers. Restrict every consumed wire ID
        # before that conversion so distinct strings cannot collapse identities.
        return type(value) is str and re.fullmatch(r"[1-9][0-9]{0,31}", value) is not None

    def route(method, url, params, body):
        if method != "GET" or body is not None or url.query:
            raise ValueError()
        if re.fullmatch(r"/2/users/by/username/[A-Za-z0-9_]{1,15}", url.path):
            if params != {"user.fields": "description,public_metrics"}:
                raise ValueError()
        elif (url.path != "/2/tweets/search/recent" or set(params) != {"query", "max_results", "tweet.fields", "user.fields", "expansions"}
              or params["expansions"] != "author_id" or not 10 <= int(params["max_results"]) <= 100):
            raise ValueError()

    def shape(payload, url, headers):
        value = _sdk_json(payload)
        if type(value) is not dict or value.get("errors") or "error" in value:
            raise ValueError()
        if url.path.startswith("/2/users/"):
            user = value.get("data")
            _sdk_strings(user, ("id", "name", "username", "description"))
            if not canonical_id(user["id"]) or user["username"].lower() != url.path.rsplit("/", 1)[1].lower():
                raise ValueError()
            _sdk_numbers(user.get("public_metrics", {}), ("followers_count", "following_count", "tweet_count"), integer=True, nonnegative=True)
        else:
            data = value.get("data")
            count = value.get("meta", {}).get("result_count")
            if type(count) is not int or not 0 <= count <= 100:
                raise ValueError()
            if "data" not in value and count == 0:
                return
            if type(data) is not list or len(data) != count:
                raise ValueError()
            includes = value.get("includes", {})
            users = includes.get("users", [])
            if type(users) is not list or len(users) > 100:
                raise ValueError()
            by_id = {}
            for user in users:
                _sdk_strings(user, ("id", "name", "username"))
                if (not canonical_id(user["id"]) or not re.fullmatch(r"[A-Za-z0-9_]{1,15}", user["username"])
                        or type(user.get("verified")) is not bool or user["id"] in by_id):
                    raise ValueError()
                by_id[user["id"]] = user
            for post in data:
                _sdk_strings(post, ("id", "text", "author_id", "created_at"))
                if not canonical_id(post["author_id"]) or post["author_id"] not in by_id or not canonical_id(post["id"]):
                    raise ValueError()
                # Validate complete wire time before the real SDK datetime model.
                from datetime import datetime
                if not re.fullmatch(r"[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9]{2}:[0-9]{2}:[0-9]{2}(?:\.[0-9]{1,6})?Z", post["created_at"]):
                    raise ValueError()
                datetime.fromisoformat(post["created_at"].replace("Z", "+00:00"))
                if include_post_metrics:
                    _sdk_numbers(post.get("public_metrics", {}), ("retweet_count", "reply_count", "like_count", "quote_count"), integer=True, nonnegative=True)

    bridge = _SdkTransport(session, timeout, "https://api.twitter.com", {"Authorization": f"Bearer {api_key}"}, route, shape)

    class SelectedClient(tweepy.Client):
        def request(self, method, route, params=None, json=None, user_auth=False):
            if user_auth:
                raise ValueError("Service request failed")
            # Do not run the SDK boundary that logs complete bearer headers.
            return bridge.request(method, "https://api.twitter.com" + route, params=params, json=json)

        def search_recent_tweets(self, *args, **kwargs):
            # The donor formats authors but omitted this required expansion.
            kwargs["expansions"] = ["author_id"]
            result = super().search_recent_tweets(*args, **kwargs)
            if result.data and len(result.data) > kwargs["max_results"]:
                raise ValueError("Service request failed")
            return result

    class SelectedXTools(XTools):
        def __init__(self):
            self.client = SelectedClient(bearer_token=api_key, wait_on_rate_limit=False)
            self.client.session.close()
            self.client.session = bridge
            self.include_post_metrics = include_post_metrics
            Toolkit.__init__(self, name="x", tools=[self.get_user_info, self.search_posts])

        def search_posts(self, query: str, max_results: int = 10) -> str:
            """Read one recent post page with real expanded authors and optional metrics."""
            value = super().search_posts(query, max_results)
            if "error" in json.loads(value):
                return json.dumps({"error": "Service request failed"})
            return value

    return SelectedXTools()


def build_youtube(timeout: int, *, session: requests.Session) -> Any:
    from urllib.parse import parse_qs
    from agno.tools import Toolkit

    class SelectedYouTubeTools(Toolkit):
        def __init__(self):
            super().__init__(name="youtube_tools", tools=[self.get_youtube_video_data])

        def get_youtube_video_data(self, url: str) -> str:
            """Read public video metadata for a supported exact YouTube URL."""
            try:
                parsed = urlsplit(url)
                if (parsed.scheme != "https" or parsed.netloc not in ("youtube.com", "www.youtube.com", "youtu.be")
                        or parsed.fragment or "\\" in url or any(ord(c) <= 32 for c in url)):
                    raise ValueError()
                query = parse_qs(parsed.query, keep_blank_values=True, strict_parsing=True)
                if parsed.netloc == "youtu.be" and not query:
                    video_id = parsed.path[1:]
                elif parsed.netloc != "youtu.be" and parsed.path == "/watch" and set(query) == {"v"} and len(query["v"]) == 1:
                    video_id = query["v"][0]
                elif parsed.netloc != "youtu.be" and not query and re.fullmatch(r"/(?:embed|v)/[A-Za-z0-9_-]{11}", parsed.path):
                    video_id = parsed.path.split("/")[2]
                else:
                    raise ValueError()
                if not re.fullmatch(r"[A-Za-z0-9_-]{11}", video_id) or "%" in url:
                    raise ValueError()
                value = _service_json(session, "GET", "https://www.youtube.com/oembed", timeout=timeout, headers={}, expected_status=200,
                                      params={"format": "json", "url": f"https://www.youtube.com/watch?v={video_id}"})
                fields = ("title", "author_name", "author_url", "type", "height", "width", "version", "provider_name", "provider_url", "thumbnail_url")
                _sdk_strings(value, tuple(key for key in fields if key not in ("height", "width")))
                _sdk_numbers(value, ("height", "width"), integer=True, nonnegative=True)
                if value["type"] != "video" or not value["title"]:
                    raise ValueError()
                return json.dumps({key: value.get(key) for key in fields}, indent=4)
            except Exception:
                return json.dumps({"error": "Service request failed"})

    return SelectedYouTubeTools()


def build_oxylabs(api_key: str, timeout: int, *, session: requests.Session, username: str) -> Any:
    from agno.tools.oxylabs import OxylabsTools
    from oxylabs.internal.api import RealtimeAPI

    class SelectedAPI(RealtimeAPI):
        def _get_http_response(self, payload, method, config):
            try:
                if method != "POST" or payload.get("source") not in ("google_search", "amazon_product") or payload.get("parse") is not True:
                    raise ValueError()
                value = _service_json(session, "POST", "https://realtime.oxylabs.io/v1/queries", timeout=timeout,
                                      headers=self._headers, body=payload, expected_status=200)
                results = value.get("results")
                if type(results) is not list or len(results) > 1 or value.get("error"):
                    raise ValueError()
                # An explicitly supplied job is usable only when the SDK's
                # documented terminal-success status is present. Otherwise an
                # empty result list could conceal a faulted or unfinished job.
                if "job" in value and (type(value["job"]) is not dict or value["job"].get("status") != "done"):
                    raise ValueError()
                for result in results:
                    if type(result) is not dict or type(result.get("status_code")) is not int or result["status_code"] != 200:
                        raise ValueError()
                    # The real SDK always constructs a Content model. Validate
                    # the branches consumed by the donor before defaults lose
                    # absence; an empty parsed list can select raw content.
                    parsed = result.get("content_parsed")
                    content = result.get("content")
                    if payload["source"] == "google_search":
                        def organic(branch):
                            if type(branch) is not dict or branch.get("_errors"):
                                raise ValueError()
                            items = branch.get("results", {}).get("organic")
                            if type(items) is not list or len(items) > 50:
                                raise ValueError()
                            for item in items:
                                _sdk_strings(item, ("title", "url", "desc"))
                                _sdk_numbers(item, ("pos",), integer=True, nonnegative=True)
                            return items

                        if parsed:
                            items = organic(parsed)
                            if not items and type(content) is dict and "results" in content:
                                organic(content)
                        else:
                            organic(content)
                    else:
                        selected = content if content else parsed
                        _sdk_strings(selected, ("title", "currency", "url", "description", "stock", "brand"))
                        if not selected["title"] or selected.get("_errors"):
                            raise ValueError()
                        _sdk_numbers(selected, ("price", "rating", "reviews_count"), nonnegative=True)
                        for field in ("images", "bullet_points"):
                            if type(selected.get(field)) is not list or any(type(item) is not str for item in selected[field]):
                                raise ValueError()
                return value
            except Exception:
                pass
            raise ValueError("Service request failed")

    class SelectedOxylabsTools(OxylabsTools):
        def _error_response(self, tool_name, error_message, context=None):
            return json.dumps({"error": "Service request failed"})

    tools = SelectedOxylabsTools(username=username, password=api_key)
    # Real SDK constructor is inert; replace only the two selected source
    # instances' API boundary, preserving their methods and Response models.
    from oxylabs.internal.api import APICredentials
    api = SelectedAPI(APICredentials(username, api_key), sdk_type="meerkat-toolkit-agno")
    tools.client.google._api_instance = api
    tools.client.amazon._api_instance = api
    return tools


def build_wikipedia(timeout: int, *, session: requests.Session, language: str) -> Any:
    from agno.knowledge.document import Document
    from agno.tools import Toolkit
    from bs4 import BeautifulSoup
    from urllib.parse import unquote

    def wikipedia_query(params):
        return _service_json(session, "GET", f"https://{language}.wikipedia.org/w/api.php", timeout=timeout,
                             params={"action": "query", "format": "json", **params},
                             headers={"User-Agent": "meerkat-toolkit-agno/0.1"}, expected_status=200)

    def page(value, expected_identity=None):
        if type(value) is not dict or any(key in value for key in ("error", "continue", "query-continue")):
            raise ValueError()
        pages = value.get("query", {}).get("pages")
        if type(pages) is not dict or len(pages) != 1:
            raise ValueError()
        page_id, result = next(iter(pages.items()))
        if type(result) is not dict or "missing" in result or "invalid" in result:
            raise ValueError()
        _sdk_strings(result, ("title",))
        if (not re.fullmatch(r"[1-9][0-9]{0,31}", page_id)
                or not 1 <= len(result["title"]) <= 256 or re.search(r"[\x00-\x1f\x7f|]", result["title"])):
            raise ValueError()
        if "pageid" in result and (type(result["pageid"]) is not int or str(result["pageid"]) != page_id):
            raise ValueError()
        identity = (page_id, result["title"])
        if expected_identity is not None and identity != expected_identity:
            raise ValueError()
        return result, identity

    def source_url(result):
        urls = []
        for field in ("canonicalurl", "fullurl"):
            if field not in result:
                continue
            value = result[field]
            if (type(value) is not str or not 1 <= len(value.encode("utf-8")) <= 4096
                    or re.search(r"[\x00-\x20\x7f\\]", value)
                    or re.search(r"%(?![0-9A-Fa-f]{2})", value)):
                raise ValueError()
            parsed = urlsplit(value)
            if (parsed.scheme != "https" or parsed.netloc != f"{language}.wikipedia.org"
                    or parsed.query or parsed.fragment or "?" in value or "#" in value
                    or not parsed.path.startswith("/wiki/")):
                raise ValueError()
            title = unquote(parsed.path[len("/wiki/"):], encoding="utf-8", errors="strict")
            if ("\\" in title or any(part in (".", "..") for part in title.split("/"))
                    or title.replace("_", " ") != result["title"].replace("_", " ")):
                raise ValueError()
            urls.append(value)
        if not urls:
            raise ValueError()
        return urls[0]

    class SelectedWikipediaTools(Toolkit):
        def __init__(self):
            super().__init__(name="wikipedia_tools", tools=[self.search_wikipedia])

        def search_wikipedia(self, query: str) -> str:
            """Read an exact Wikipedia title summary or disambiguation options."""
            try:
                title = query
                redirect_count = 0
                # Attributable port of wikipedia 1.4.0 page/summary: HTTPS and
                # bounded logical redirects, no global locale/cache or suggest.
                for attempt in range(4):
                    value = wikipedia_query({"prop": "info|pageprops", "inprop": "url", "ppprop": "disambiguation", "redirects": "", "titles": title})
                    result, identity = page(value)
                    redirects = value.get("query", {}).get("redirects", [])
                    if type(redirects) is not list or len(redirects) > 3:
                        raise ValueError()
                    redirect_count += len(redirects)
                    if redirects:
                        for item in redirects:
                            _sdk_strings(item, ("from", "to"))
                        if redirects[-1]["to"] != result["title"] or redirect_count > 3 or attempt == 3:
                            raise ValueError()
                        title = result["title"]
                        continue
                    if "disambiguation" in result.get("pageprops", {}):
                        parsed, _ = page(wikipedia_query({"prop": "revisions", "rvprop": "content", "rvparse": "", "rvlimit": 1, "titles": result["title"]}), identity)
                        revisions = parsed.get("revisions")
                        if type(revisions) is not list or len(revisions) != 1:
                            raise ValueError()
                        html = revisions[0].get("*")
                        if type(html) is not str:
                            raise ValueError()
                        soup = BeautifulSoup(html, "html.parser")
                        options = [li.a.get_text() for li in soup.find_all("li") if li.a and "tocsection" not in " ".join(li.get("class", []))]
                        if not options or len(options) > 50:
                            raise ValueError()
                        return json.dumps({"disambiguation": query, "options": options})
                    summary, summary_identity = page(wikipedia_query({"prop": "extracts|info", "inprop": "url", "explaintext": "", "exintro": "", "titles": result["title"]}), identity)
                    content = summary.get("extract")
                    if type(content) is not str or not content:
                        raise ValueError()
                    metadata = {
                        "source_url": source_url(summary),
                        "canonical_title": summary["title"],
                        "page_id": int(summary_identity[0]),
                        "language": language,
                        # Observation time is not the content's modification or
                        # revision time. TextExtracts does not bind an extract
                        # to the page revision reported by prop=info.
                        "retrieved_at": datetime.now(timezone.utc).isoformat().replace("+00:00", "Z"),
                    }
                    return json.dumps(Document(name=query, content=content, meta_data=metadata).to_dict())
                raise ValueError()
            except Exception:
                return json.dumps({"error": "Service request failed"})

    return SelectedWikipediaTools()


def build_aws_lambda(api_key: str, timeout: int, *, session: requests.Session,
                     access_key_id: str, session_token: str | None, region: str, resources: Any) -> Any:
    import botocore.session
    from botocore.auth import SigV4Auth, SIGV4_TIMESTAMP, get_current_datetime
    from botocore.awsrequest import AWSRequest, AWSResponse
    from botocore.config import Config
    from botocore.configprovider import BOTOCORE_DEFAUT_SESSION_VARIABLES
    from botocore.credentials import Credentials
    from agno.tools import Toolkit
    from agno.tools.aws_lambda import AWSLambdaTools

    # All SDK configuration sources are explicit. No shared credentials/config,
    # SDK endpoint overrides, IMDS, container credentials or ambient profiles.
    variables = {key: (None, None, default, conversion) for key, (_, _, default, conversion) in BOTOCORE_DEFAUT_SESSION_VARIABLES.items()}
    variables["config_file"] = (None, None, "/dev/null", None)
    variables["credentials_file"] = (None, None, "/dev/null", None)
    sdk = botocore.session.Session(session_vars=variables)
    client = sdk.create_client("lambda", region_name=region, aws_access_key_id=access_key_id,
                               aws_secret_access_key=api_key, aws_session_token=session_token,
                               endpoint_url=f"https://lambda.{region}.amazonaws.com",
                               config=Config(retries={"total_max_attempts": 1}, proxies={},
                                             connect_timeout=timeout, read_timeout=timeout,
                                             ignore_configured_endpoint_urls=True))
    resources.callback(client.close)
    credentials = Credentials(access_key_id, api_key, session_token)

    class SelectedSigner(SigV4Auth):
        def add_auth(self, request):
            # Narrow port of add_auth omits logging the canonical credential and
            # session-token headers; real SDK canonicalization/HMAC remain.
            request.context["timestamp"] = get_current_datetime().strftime(SIGV4_TIMESTAMP)
            self._modify_request_before_signing(request)
            canonical = self.canonical_request(request)
            signature = self.signature(self.string_to_sign(request, canonical), request)
            self._inject_signature_to_request(request, signature)

    def make_request(operation_model, request_dict):
        try:
            if operation_model.name != "ListFunctions":
                raise ValueError()
            request = AWSRequest(method=request_dict["method"], url=request_dict["url"],
                                 data=request_dict["body"], headers=request_dict["headers"])
            parsed = urlsplit(request.url)
            if (request.method != "GET" or parsed.netloc != f"lambda.{region}.amazonaws.com"
                    or parsed.scheme != "https" or parsed.path != "/2015-03-31/functions"
                    or parsed.query != "MaxItems=50" or request.data):
                raise ValueError()
            SelectedSigner(credentials, "lambda", region).add_auth(request)
            headers = {}
            payload = _service_bytes(session, "GET", request.url, timeout=timeout,
                                     headers=dict(request.headers), expected_status=200, response_headers=headers)
            value = _sdk_json(payload)
            functions = value.get("Functions")
            if type(functions) is not list or len(functions) > 50 or ("NextMarker" in value and value["NextMarker"] not in (None, "")) or "error" in value:
                raise ValueError()
            for function in functions:
                _sdk_strings(function, ("FunctionName",))
                if not re.fullmatch(r"[A-Za-z0-9_-]{1,64}", function["FunctionName"]):
                    raise ValueError()
            # Use the real botocore protocol parser and operation output model.
            result = client._response_parser.parse({"headers": headers, "status_code": 200, "body": payload}, operation_model.output_shape)
            return AWSResponse(request.url, 200, headers, None), result
        except Exception:
            pass
        raise ValueError("Service request failed")

    client._endpoint.make_request = make_request
    original_list = client.list_functions

    def list_functions():
        return original_list(MaxItems=50)

    client.list_functions = list_functions

    class SelectedLambdaTools(AWSLambdaTools):
        def __init__(self):
            self.client = client
            Toolkit.__init__(self, name="aws-lambda", tools=[self.list_functions])

    return SelectedLambdaTools()


def build_postgres(*, socket_directory: str, port: int, database: str, user: str,
                   password: str, schema: str, timeout: int, max_rows: int) -> Any:
    """PostgresTools connection/helper port; the two operation bodies are inherited.

    Donor: Agno 3.0.11 postgres.py, SHA-256
    94be4e1ed53fe16b5f56ded1c023f6af9ab77bdf852730ff97a87d3dfa380b62.
    A lazy owned psycopg connection replaces ambient libpq configuration. A
    server cursor replaces fetchall, and csv.writer replaces ambiguous joining.
    Fixed helper failures raise after rollback instead of logging/returning raw
    exception strings. Remote connections are deliberately outside this profile.
    """
    import csv
    import io
    import os

    import psycopg
    from psycopg.rows import tuple_row
    from agno.tools.postgres import PostgresTools

    def reject_ambient() -> None:
        # libpq ignores empty explicit values and can then consult PG* variables.
        # Refuse that state; never clear or rewrite process-global environment.
        if any(key.startswith("PG") for key in os.environ):
            raise ValueError("PostgreSQL profile cannot use ambient libpq state")

    reject_ambient()

    class SelectedPostgres(PostgresTools):
        def __init__(self) -> None:
            self._retired = False
            super().__init__(table_schema=schema)

        def connect(self) -> Any:
            if self._retired:
                raise ValueError("PostgreSQL profile is closed")
            if self._connection is not None:
                if self._connection.closed:
                    raise ValueError("PostgreSQL connection is closed")
                return self._connection
            reject_ambient()
            connection = None
            try:
                connection = psycopg.connect(
                    host=socket_directory, port=port, dbname=database, user=user,
                    password=password, passfile="/dev/null", connect_timeout=timeout,
                    sslmode="disable", gssencmode="disable", require_auth="scram-sha-256",
                    client_encoding="UTF8", application_name="meerkat-toolkit-postgres",
                    options=("-c search_path=pg_catalog -c default_transaction_read_only=on "
                             f"-c statement_timeout={timeout * 1000} -c lock_timeout={timeout * 1000} "
                             f"-c idle_in_transaction_session_timeout={timeout * 1000}"),
                    row_factory=tuple_row,
                )
                connection.read_only = True
                self._connection = connection
                return connection
            except Exception:
                if connection is not None:
                    connection.close()
            raise ValueError("PostgreSQL connection failed")

        def close(self) -> None:
            self._retired = True
            super().close()

        def _execute_query(self, query: str, params: tuple | None = None) -> str:
            # Only these actual donor metadata statements are admitted by this
            # helper port. The public adapter also selects only the two reads.
            normalized = " ".join(query.split())
            tables = "SELECT table_name FROM information_schema.tables WHERE table_schema = %s;"
            columns = ("SELECT column_name, data_type, is_nullable FROM information_schema.columns "
                       "WHERE table_schema = %s AND table_name = %s;")
            expected = (("table_name",) if normalized == tables else
                        ("column_name", "data_type", "is_nullable") if normalized == columns else None)
            if (expected is None or type(params) is not tuple or len(params) != (1 if normalized == tables else 2)
                    or params[0] != schema):
                raise ValueError("Unsupported PostgreSQL metadata request")
            if len(params) == 2:
                table = params[1]
                if type(table) is not str or not 1 <= len(table.encode("utf-8")) <= 63 or "\x00" in table:
                    raise ValueError("Invalid PostgreSQL table identifier")
            connection = None
            failed = False
            value = None
            try:
                connection = self.connect()
                with connection.cursor(name="toolkit_metadata", row_factory=tuple_row) as cursor:
                    cursor.execute(query, params)
                    if cursor.description is None or tuple(item.name for item in cursor.description) != expected:
                        raise ValueError()
                    # A server cursor bounds rows transferred/materialized, not
                    # merely the list length after a client-side fetchall.
                    rows = cursor.fetchmany(max_rows + 1)
                    if len(rows) > max_rows:
                        raise ValueError()
                    if any(type(row) is not tuple or len(row) != len(expected)
                           or any(type(cell) is not str or len(cell.encode("utf-8")) > 256 for cell in row)
                           for row in rows):
                        raise ValueError()
                    if not rows:
                        value = f"Query returned no results.\nColumns: {', '.join(expected)}"
                    else:
                        output = io.StringIO(newline="")
                        writer = csv.writer(output, lineterminator="\n")
                        writer.writerow(expected)
                        writer.writerows(rows)
                        value = output.getvalue()[:-1]
                        if len(value.encode("utf-8")) > 1_048_576:
                            raise ValueError()
            except Exception:
                failed = True
            finally:
                if connection is not None:
                    try:
                        connection.rollback()
                    except Exception:
                        failed = True
                        connection.close()
            if failed:
                raise ValueError("PostgreSQL metadata operation failed")
            return value

    return SelectedPostgres()
