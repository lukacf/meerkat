"""Inert newcomer guidance for the selected Agno profiles.

These are maintained descriptions of the pinned adapter, its guide and dated
provider documentation, not discovery, auth state or evidence that an account works.
The existing profile validator owns selection and configuration semantics.
"""
from __future__ import annotations

import json
from importlib.resources import files


def selected_configuration_guide(profile: str) -> dict[str, str]:
    """Present a build-time projection of the package's exact retained manual.

    The existing validator owns semantics. This lookup neither extracts Markdown
    nor chooses a profile; missing generated guidance is a package error.
    """
    source = json.loads(files(__package__).joinpath("CONFIGURATION-GUIDES.json").read_text(encoding="utf-8"))
    return {"profile": profile, "full_guide_sha256": source["full_guide_sha256"],
            "text": "\n\n".join(source["sections"][key] for key in source["profiles"][profile])}


_LOCAL_MCP = (
    "This profile is a local stdio MCP server, not a remote MCP OAuth endpoint. "
    "The host owns access to its child process. Any selected service credential "
    "is for the local connector's provider connection; never send it as bearer "
    "authentication to an arbitrary MCP server."
)
_INJECTION = (
    "Have the host credential owner inject only the selected credential references "
    "into the child process. Configuration contains environment-variable names, "
    "never secret values. Obtain missing credentials through that owner's approved "
    "path outside model/tool transcripts. This component does not acquire, refresh "
    "or revoke credentials, register applications, or launch authentication."
)

_PUBLIC = {
    "hackernews": "Public Hacker News reads require no provider credential in this profile.",
    "pubmed": "Public PubMed reads require no provider credential. The required email is an operator-supplied contact address, not an account credential.",
    "searxng": "This profile sends unauthenticated reads to the explicitly configured SearXNG host. It has no credential field or login path for a protected instance.",
    "arxiv": "Public arXiv feed reads require no provider credential in this profile.",
    "youtube": "Public YouTube oEmbed metadata requires no provider credential. This is not a Google OAuth, browser, transcript or video-download profile.",
    "wikipedia": "Public Wikipedia reads require no provider credential. Choose the explicit language; it is not an account selector.",
}

def authentication_metadata(profile: str) -> dict[str, dict[str, str]]:
    """Publish only the explicit public-profile contract, never infer from fields."""
    return {"authentication": {"kind": "none"}} if profile in _PUBLIC else {}


# Reference meanings follow the selected adapter, not the shared api_key_env name.
_CREDENTIALS = {
    "tavily": "api_key_env names the selected Tavily account API key.",
    "financial-datasets": "api_key_env names the selected Financial Datasets account API key.",
    "openweather": "api_key_env names the selected OpenWeather account API key.",
    "bitbucket": "api_key_env names a Bitbucket Cloud API token, paired with the explicit account_email for HTTP Basic authentication. It is not an account password.",
    "gitlab": "api_key_env names a GitLab personal access token accepted as PRIVATE-TOKEN, not an OAuth client secret.",
    "clickup": "api_key_env names the selected ClickUp API key; workspace_id is an explicit resource selection.",
    "discord": "api_key_env names a Discord bot token, not a user password or an MCP bearer credential.",
    "shopify": "api_key_env names the selected shop's Shopify Admin API token; shop_name and api_version are explicit configuration.",
    "spotify": "api_key_env names an already acquired Spotify OAuth access token, not an API key, client secret or refresh token.",
    "unsplash": "api_key_env names the selected Unsplash API access key.",
    "calcom": "api_key_env names the selected Cal.com API key.",
    "jina": "api_key_env names the selected Jina Reader API key.",
    "openrouteservice": "api_key_env names the selected OpenRouteService API key.",
    "zendesk": "api_key_env names the selected Zendesk credential. Set auth_mode=oauth_access_token for an existing OAuth access token sent as Bearer; omit account_email in that mode. Omitted auth_mode or auth_mode=api_token retains legacy Basic authentication and requires account_email, combined with /token. The connector never guesses the credential type or falls back between modes.",
    "bright-data": "api_key_env names the selected Bright Data API credential used as a bearer token; web_unlocker_zone and serp_zone select its explicit zones.",
    "linkup": "api_key_env names the selected Linkup API key.",
    "todoist": "api_key_env names the selected Todoist API token used by the existing SDK client.",
    "zoom": "api_key_env names an already acquired Zoom bearer token. No OAuth endpoint or refresh path is part of this profile.",
    "exa": "api_key_env names the selected Exa API key.",
    "firecrawl": "api_key_env names the selected Firecrawl API key.",
    "trello": "api_key_env names the Trello API key and token_env names its separately selected user token. Both are required; the SDK sends a key/token pair, not a fabricated OAuth signature.",
    "salesforce": "api_key_env names an existing Salesforce session token for the explicit instance_url. The profile does not perform password login.",
    "twilio": "api_key_env names the Twilio auth token paired with the selected account_sid for HTTP Basic authentication.",
    "webex": "api_key_env names an already acquired Webex access token.",
    "plivo": "api_key_env names the Plivo auth token paired with the selected auth_id for HTTP Basic authentication.",
    "reddit": "api_key_env names an existing Reddit OAuth access token. user_agent supplies the required contact/application identity, not credentials.",
    "x": "api_key_env names the existing X bearer token. OAuth1 user-context credentials and token refresh are outside this profile.",
    "oxylabs": "api_key_env names the selected Oxylabs account password, paired with username for HTTP Basic authentication. Despite its field name, this value is not an API key.",
    "aws-lambda": "api_key_env names the AWS secret access key and access_key_id_env names its matching access key ID. session_token_env optionally names the matching session token; when configured it must be supplied. These are explicit signing credentials, not an MCP bearer token.",
    "postgres": "password_env names the selected PostgreSQL principal's password for SCRAM SHA-256 authentication over the configured local Unix socket. It is not an API key or a connection URI.",
}

_CONTEXT = {
    "pubmed": (("account", "Supply the contact email required by the selected PubMed profile; no account login is implemented."),),
    "searxng": (("host_deployment", "Supply an operator-approved SearXNG host that permits these unauthenticated reads. The host owns its network, proxy and CA policy."),),
    "financial-datasets": (("account", "The account key must have the dataset entitlement for the selected income-statement and balance-sheet reads. Account setup and billing belong to the account owner."),),
    "openweather": (("account", "Obtain an OpenWeather account key with access to the selected current-weather, 5-day-forecast, air-pollution and direct-geocoding endpoints. Key activation, plan access and billing remain account-owner facts."),),
    "bitbucket": (("account", "Select the account email, workspace and repository explicitly. The retained guide identifies read:repository:bitbucket and, for pull-request reads, read:pullrequest:bitbucket; the token must also access the selected resources."),),
    "gitlab": (("account", "The documented setup uses a personal access token with read_api and access to the selected projects, merge requests and issues. Supply the exact HTTPS origin for self-managed GitLab; URL subpaths are unsupported."),),
    "clickup": (("account", "Supply the selected workspace and its account API key; a workspace_id alone does not establish account access."),),
    "discord": (("account", "The selected bot token must have access to the requested channel or guild. These reads do not publish an agent frontend."),),
    "shopify": (("account", "Select the shop and an Admin API token with access to products for product reads; shop configuration alone does not prove that permission."),),
    "spotify": (("account", "The retained donor guide identifies user-top-read for top-track reads. Playlist visibility follows the selected token; construction does not verify the selected account or scopes."),),
    "calcom": (("account", "Supply the selected API key, numeric event_type_id and IANA user_timezone. The event type does not filter get_upcoming_bookings; that operation reads the selected account's upcoming bookings."),),
    "zendesk": (("account", "Select the Help Center subdomain explicitly. Legacy api_token mode also requires account_email; oauth_access_token mode rejects it because the token determines the acting identity. This profile reads Help Center articles and does not supply ticket access or a frontend."),),
    "bright-data": (("account", "Select the Web Unlocker and SERP zones explicitly for this account; do not rely on ambient zone defaults."),),
    "trello": (("account", "Have the account owner provide both the selected API key and user token for board reads. Configuring the two reference names does not verify that they belong together or have the needed board visibility."),),
    "salesforce": (("account", "Select the Salesforce instance explicitly and supply an existing session token that can read the requested object/record. No password login, refresh or SOQL path is supplied."),),
    "twilio": (("account", "Select account_sid and its matching auth token for call-detail reads; this profile cannot provision accounts, send messages or place calls."),),
    "plivo": (("account", "Select auth_id and its matching auth token for call-detail reads; this profile cannot provision accounts, send messages or place calls."),),
    "reddit": (("account", "Supply the selected OAuth access token and user_agent with contact/application identity. SDK credential discovery and authentication acquisition are disabled."),),
    "oxylabs": (("account", "Supply the selected username and its account password for the fixed Oxylabs realtime API. Keep that password in the host credential path, never in the JSON configuration or a tool argument."),),
    "aws-lambda": (("account", "Supply a matching explicit AWS credential set and commercial region with permission for the selected Lambda list operation. Shared credential files, ambient profiles, metadata credentials, STS acquisition and refresh are not provided."),),
    "postgres": (
        ("administrator", "A database operator must provision a principal with the intended metadata visibility and SCRAM SHA-256 password authentication. This profile does not create roles, acquire credentials or provision schemas."),
        ("host_deployment", "Select a reachable local Unix socket, port, database, user and schema. TCP, remote TLS, trust/peer authentication, service files and password-file fallback are unsupported. The password reference name cannot start with PG, and PG-prefixed environment variables are refused."),
    ),
}

# Static provider descriptions; no registration, policy or credential execution.
_PROVIDER_GUIDANCE = {'aws-lambda': (('developer_registration',
                 'Reviewed 2026-10-03. Have the AWS account/security owner provide an approved explicit '
                 "credential set. Prefer short-lived credentials acquired by the host's existing role or "
                 'federation workflow, including their matching session token. If a long-lived IAM-user key '
                 'is approved instead, use IAM, Users, the selected user, Security credentials, Create '
                 'access key; save the secret once through host custody and never use a root key. Configure '
                 'references for the access key ID and secret, plus session_token_env whenever the supplied '
                 'credentials are temporary. The connector does not run STS, discover a role, read shared '
                 'profiles or refresh credentials.'),
                ('account',
                 'list_functions requires lambda:ListFunctions. This list action does not support a function '
                 "ARN resource restriction, so the IAM allow uses Resource='*'; account/region controls and "
                 'applicable policy conditions can still constrain access. No lambda:InvokeFunction, '
                 'lambda:GetFunction, IAM PassRole or write grant is required. Identity policies, '
                 'permissions boundaries, SCPs and session policies can still deny the read. Select a '
                 'supported commercial AWS region explicitly; GovCloud/China partitions and arbitrary '
                 'endpoint overrides are not supplied by this profile. The connector rejects a response '
                 'requiring a further page, so this is not an unbounded account inventory.'),
                ('developer_registration',
                 'Provider setup references checked 2026-10-03: '
                 'https://docs.aws.amazon.com/lambda/latest/api/API_ListFunctions.html ; '
                 'https://docs.aws.amazon.com/service-authorization/latest/reference/list_lambda.html ; '
                 'https://docs.aws.amazon.com/IAM/latest/UserGuide/id_credentials_access-keys.html ; '
                 'https://docs.aws.amazon.com/IAM/latest/UserGuide/access-keys-admin-managed.html ; '
                 'https://docs.aws.amazon.com/IAM/latest/UserGuide/securing_access-keys.html'),
                ('The intended AWS principal/account, effective allow and deny policies, commercial region, '
                 'matching session token and expiration remain account-owner facts.',)),
 'bitbucket': (('developer_registration',
                'Reviewed 2026-10-03. In the intended Atlassian account, open Account settings, Security, '
                'Create and manage API tokens, then Create API token with scopes. Choose Bitbucket, a name '
                'and expiry, and only the scopes needed below. Keep the one-time token in the host '
                "credential store. Pair it with that account's email for Basic authentication; this route "
                'does not use an account password, legacy app password or workspace access-token Bearer '
                'authentication.'),
               ('account',
                'For list_repositories or get_repository_details, select read:repository:bitbucket. For '
                'list_all_pull_requests or get_pull_request_details, select read:pullrequest:bitbucket; add '
                'repository read when repository operations are also selected. API-token scopes do not '
                'implicitly grant one another. The PR read scope itself also permits commenting according to '
                'Atlassian, although this connector only reads. The account must already see the chosen '
                'workspace/repository; a configured repo_slug does not grant access. This route returns '
                'bounded first pages, not a complete workspace inventory.'),
               ('developer_registration',
                'Provider setup references checked 2026-10-03: '
                'https://support.atlassian.com/bitbucket-cloud/docs/create-an-api-token/ ; '
                'https://support.atlassian.com/bitbucket-cloud/docs/api-token-permissions/ ; '
                'https://developer.atlassian.com/cloud/bitbucket/rest/'),
               ("The selected account's workspace/repository membership, organizational token restrictions, "
                'token expiry and actual selected-operation scopes remain account-owner facts.',)),
 'bright-data': (('developer_registration',
                  'Have a Bright Data account administrator open Account settings, Users and API keys, then '
                  'Add API key. Select the intended user, API-usage permission and expiry; retain the '
                  'displayed key in the host credential store. In Web Access, Create API creates named Web '
                  'Unlocker or SERP zones. Complete any account verification requested there.'),
                 ('account',
                  'scrape_as_markdown needs the selected web_unlocker_zone; search_engine needs the selected '
                  'serp_zone. Both use the regular REST API key with /request. The User permission permits '
                  'API usage without granting billing or product administration. Both zone names are '
                  'mandatory configuration fields, but only selected operations require their corresponding '
                  'working product. A proxy password or separately issued Agent API key is not proven '
                  'interchangeable with this REST credential.'),
                 ('developer_registration',
                  'Provider setup references checked 2026-10-03: '
                  'https://docs.brightdata.com/api-reference/authentication ; '
                  'https://docs.brightdata.com/products/web-unlocker/quickstart ; '
                  'https://docs.brightdata.com/products/serp-api/quickstart'),
                 ('The account administrator must verify key permission and expiry, selected zone ownership, '
                  'product activation, target restrictions and current allowance. The documentation '
                  "describes differing free-tier verification cases; the account's actual enrollment "
                  'requirements remain unverified.',)),
 'calcom': (('developer_registration',
             'Reviewed 2026-10-04. On the intended Cal.com account, open Settings, Security and '
             'create/manage an API key, then store it through the host credential owner. Select the actual '
             'event type ID and IANA timezone. This route uses Bearer API-key authentication; it does not '
             'register or refresh OAuth clients. Cal.com recommends OAuth for integrations and requires a '
             'verified OAuth client for an App Store partner. Its deprecated Platform offering is not a '
             'new-customer prerequisite for this account-key route.'),
            ('account',
             'get_available_slots reads availability for event_type_id. get_upcoming_bookings reads the '
             "selected account's upcoming bookings and can optionally filter attendeeEmail; event_type_id "
             'does not narrow that operation. The API key carries broad account privileges, not a '
             'two-operation read-only grant. Calendar connections, schedules and event-type availability '
             'must exist in the account. Availability uses GET /v2/slots with cal-api-version 2024-09-04, '
             'format=range and UTC query boundaries, then formats starts in the configured timezone. '
             'Bookings use GET /v2/bookings with cal-api-version 2026-05-01 and limit=100. Only the first '
             'page is read; a hasMore response is explicitly marked incomplete and its cursor is not '
             'followed. Dates and displayed timestamps remain limited to 1970-2037, with at most 31 '
             'inclusive UTC dates per availability request. Provider documentation establishes these '
             'request contracts; it does not verify live access for the selected account.'),
            ('developer_registration',
             'Provider setup references checked 2026-10-04: '
             'https://cal.com/docs/api-reference/v2/introduction ; '
             'https://cal.com/docs/api-reference/v2/slots/get-available-time-slots-for-an-event-type ; '
             'https://cal.com/docs/api-reference/v2/bookings/get-all-bookings ; '
             'https://cal.com/docs/api-reference/v2/oauth'),
            ("The account's API-key availability, plan/policy, event-type ownership, calendar connections "
             'and actual availability/bookings access remain account-owner facts.',)),
 'clickup': (('developer_registration',
              "Reviewed 2026-10-03. For this personal-token route, open the intended ClickUp user's avatar, "
              'Settings, Apps and generate the personal API token. Store it through the host credential '
              "owner. The connector sends it directly in Authorization. ClickUp's OAuth app registration is "
              'a separate route for applications used by other people, requiring a Workspace owner or admin '
              'to register the app; this profile does not register an OAuth client or run that flow.'),
             ('account',
              'get_task reads the specified task and list_spaces reads spaces under workspace_id (called '
              'team_id in the API). A personal token has the same access as its user rather than a '
              'selectable read-only scope for these methods. Choose a user permitted to see the '
              'task/Workspace and private spaces; workspace_id is a resource selector, not a token '
              'restriction. This connector has no task-write operation, but the underlying personal token '
              'can have broader authority.'),
             ('developer_registration',
              'Provider setup references checked 2026-10-03: '
              'https://developer.clickup.com/docs/authentication ; '
              'https://developer.clickup.com/reference/gettask ; '
              'https://developer.clickup.com/reference/getspaces'),
             ("The user's membership, guest/private-resource access, personal-token availability under "
              'Workspace policy and chosen task/Workspace remain account-owner facts.',)),
 'discord': (('developer_registration',
              'Reviewed 2026-10-03. Create or select an application in the Discord Developer Portal, use its '
              'Bot page to obtain/reset the bot token, and place that token in host custody. Install the bot '
              'in the intended server using a Guild Install link with the bot scope; an installing member '
              'needs Manage Guild. Request only required bot permissions, not Administrator. This is a REST '
              'connector, not an agent frontend, user self-bot, slash-command server or Gateway listener.'),
             ('account',
              'get_channel_info and list_channels require the bot to have access to the addressed '
              'channel/guild; listing guild channels does not include threads. For get_channel_messages in a '
              'guild channel, grant View Channel and Read Message History through the bot role and channel '
              'overrides; voice-channel message access also requires Connect. To receive message content, '
              "embeds, attachments and components where Discord's exceptions do not apply, enable Message "
              'Content intent and obtain approval when required for a verified application. Without it the '
              'API can return empty content fields. This route does not need Send Messages or Manage '
              'Messages, and a successful metadata read does not prove history/content access.'),
             ('developer_registration',
              'Provider setup references checked 2026-10-03: '
              'https://docs.discord.com/developers/quick-start/getting-started ; '
              'https://docs.discord.com/developers/resources/message ; '
              'https://docs.discord.com/developers/resources/guild ; '
              'https://docs.discord.com/developers/events/gateway#message-content-intent'),
             ('The actual server installation, role/channel overrides, private-thread access if addressed, '
              'and Message Content approval/configuration remain application/server-owner facts.',)),
 'exa': (('developer_registration',
          'Create or sign in to the selected Exa account, open its dashboard and create an API key. Have the '
          'host credential owner bind api_key_env. This profile uses the direct API key even where Exa '
          'offers a separate keyless hosted MCP route.'),
         ('account',
          'search_exa calls /search and can request result text through the same search call. Confirm Search '
          'access and credits for that request. This selected adapter does not expose separate Contents, '
          'Agent, Websets, monitor or key-management operations; it requests no OAuth scope or callback.'),
         ('developer_registration',
          'Provider setup references checked 2026-10-03: https://exa.ai/docs/get-started/quickstart ; '
          'https://exa.ai/docs/reference/search'),
         ("The key's team/account, Search allowance and remaining credits are not verified. Provider key "
          'authority may be broader than the operations exposed here.',)),
 'financial-datasets': (('developer_registration',
                         'Sign up or sign in at financialdatasets.ai and generate an API key in the account '
                         'dashboard. Have the host credential owner supply that key through api_key_env. The '
                         'selected transport sends X-API-KEY, not an OAuth access token.'),
                        ('account',
                         'get_income_statements reads /financials/income-statements; get_balance_sheets '
                         'reads /financials/balance-sheets. Confirm that the selected account permits the '
                         'requested dataset, tickers, reporting periods and volume. These reads do not need '
                         'an OAuth application or redirect URL.'),
                        ('developer_registration',
                         'Provider setup references checked 2026-10-03: '
                         'https://docs.financialdatasets.ai/quickstart ; '
                         'https://docs.financialdatasets.ai/api/financials/income-statements ; '
                         'https://docs.financialdatasets.ai/api/financials/balance-sheets'),
                        ("The account's active key, dataset coverage, paid entitlements and remaining limits "
                         'require account-owner verification.',)),
 'firecrawl': (('developer_registration',
                'Create or sign in to a Firecrawl cloud account and obtain an API key from its dashboard. '
                'Store it with the host credential owner for api_key_env. This pinned profile requires a key '
                'even where current Firecrawl offers keyless requests; its origin is the hosted '
                'api.firecrawl.dev service.'),
               ('account',
                "scrape_website uses /v2/scrape and map_website uses /v2/map. Check the selected team's "
                'allowance for each enabled endpoint. These operations submit URLs to Firecrawl; they do not '
                'supply a website login session. No OAuth client or redirect URL is requested by this '
                'API-key route, and a self-hosted service is not selected by this profile.'),
               ('developer_registration',
                'Provider setup references checked 2026-10-03: https://docs.firecrawl.dev/introduction ; '
                'https://docs.firecrawl.dev/api-reference/endpoint/scrape ; '
                'https://docs.firecrawl.dev/api-reference/endpoint/map'),
               ("The key's team, enabled endpoint access, current credits/limits and authority to submit the "
                'target URLs remain unverified.',)),
 'gitlab': (('developer_registration',
             "Reviewed 2026-10-03. On the selected GitLab host, open the user's profile settings, Access, "
             'Personal access tokens, and create a legacy personal access token with a name and expiry. '
             'Current GitLab distinguishes legacy from fine-grained tokens. Put the PAT in the host '
             'credential store for PRIVATE-TOKEN authentication. An enterprise group or instance '
             'administrator can disable PAT use or limit lifetime. Supply an HTTPS origin, without a URL '
             'subpath, for self-managed GitLab.'),
            ('account',
             'The selected list_projects, get_project, list_merge_requests, get_merge_request and '
             "list_issues calls are covered by read_api plus the user's resource permissions. "
             'read_repository alone does not cover these general API reads. Merge-request and issue methods '
             'first look up their project, so a replacement fine-grained grant would also need that project '
             'read; this profile does not derive a fine-grained permission set. Do not request api write '
             'access merely to use these reads. Private projects and confidential issues remain subject to '
             'GitLab membership and visibility rules.'),
            ('developer_registration',
             'Provider setup references checked 2026-10-03: '
             'https://docs.gitlab.com/user/profile/personal_access_tokens/ ; '
             'https://docs.gitlab.com/security/tokens/access_token_scopes/ ; '
             'https://docs.gitlab.com/user/permissions/'),
            ("The host version, PAT policy/expiry, selected user's project roles and access to confidential "
             'issues must be verified for the intended account.',)),
 'jina': (('developer_registration',
           "Open Jina Reader's API Key & Billing page, obtain the selected Jina API key and retain it with "
           'the host credential owner. Signing in allows management of the key. Supply its reference as '
           'api_key_env; this profile does not implement the separate hosted Jina MCP setup.'),
          ('account',
           'read_url sends the selected URL to r.jina.ai; search_query sends the query to s.jina.ai. Both '
           'selected operations use the key even though Jina also offers keyless Reader access. Search '
           'requires a key and consumes tokens. One Jina key can authorize other Jina products too; '
           'selecting these operations narrows this adapter, not the provider credential. No '
           'destination-site login or cookie grant is supplied.'),
          ('developer_registration', 'Provider setup references checked 2026-10-03: https://jina.ai/reader/'),
          ("The selected key's balance, tier and live limits, and permission to send the chosen URLs or "
           'queries to Jina, remain host/account facts.',)),
 'linkup': (('developer_registration',
             'Create or sign in to a Linkup account using its Get Started portal and obtain the account API '
             "key. Supply that key through the host's api_key_env reference. This selected adapter uses the "
             'API-key route, not the separate x402 payment workflow.'),
            ('account',
             'web_search_with_linkup calls /v1/search with the explicitly configured standard or deep depth '
             'and searchResults or sourcedAnswer output. Confirm Search access and allowance for the '
             'selected depth. No OAuth application, callback, wallet or payment signature is required by '
             'this key-based route.'),
            ('developer_registration',
             'Provider setup references checked 2026-10-03: '
             'https://docs.linkup.so/pages/documentation/get-started/quickstart ; https://www.linkup.so/ ; '
             'https://www.linkup.so/blog/linkup-web-search-for-ai-agents-no-human-account-needed'),
            ("The key's account, Search entitlement, selected-depth allowance and remaining usage are not "
             'established by help or config validation.',)),
 'openrouteservice': (('developer_registration',
                       'Use the HeiGIT account portal at account.heigit.org to register, complete account '
                       'verification and obtain the selected API key. Store it with the host credential '
                       'owner and refer to it through api_key_env. Existing OpenRouteService keys are '
                       'documented to work on the replacement service; this does not verify any particular '
                       'account.'),
                      ('account',
                       'geocode_location needs geocoding access. get_directions performs two geocoding '
                       'requests before its directions request, so it needs both services even when '
                       'geocode_location is not separately exposed. This transport uses the fixed '
                       'https://api.heigit.org origin: GET /pelias/v1/search and POST '
                       '/openrouteservice/v2/directions/{profile}. Compared with the earlier '
                       'api.openrouteservice.org artifact, this changes the egress origin. The host must '
                       'permit the replacement origin under its existing network policy before use. No '
                       'legacy-host fallback or arbitrary-host override is supplied.'),
                      ('developer_registration',
                       'Provider setup references checked 2026-10-03: https://account.heigit.org/ ; '
                       'https://ask.openrouteservice.org/t/deprecating-api-openrouteservice-org-in-favour-of-api-heigit-org/7912 '
                       '; '
                       'https://ask.openrouteservice.org/t/reducing-the-quota-of-deprecated-api-api-openrouteservice-org/8013'),
                      ("The selected key's validity, account entitlement, remaining quota and live service "
                       'availability are unverified. Host permission for the replacement origin is required; '
                       'changing a credential reference does not establish that permission.',)),
 'openweather': (('developer_registration',
                  'Create an OpenWeather account, complete email verification and retrieve or generate a key '
                  "in the account's API keys page. New keys can take up to two hours after registration to "
                  'activate. Have the host credential owner bind api_key_env after activation; no OAuth '
                  'client or callback is involved.'),
                 ('account',
                  'Match the subscription to each selected endpoint: get_current_weather uses '
                  '/data/2.5/weather, get_forecast uses /data/2.5/forecast, get_air_pollution uses '
                  '/data/2.5/air_pollution, and geocode_location uses /geo/1.0/direct. Forecast here means '
                  'the 5-day endpoint, not One Call or historical weather. Usage limits apply across the '
                  "account's keys; another key is not another quota."),
                 ('developer_registration',
                  'Provider setup references checked 2026-10-03: https://openweathermap.org/appid ; '
                  'https://openweathermap.org/faq ; https://openweathermap.org/api/current ; '
                  'https://openweathermap.org/forecast5 ; https://openweathermap.org/api/air-pollution ; '
                  'https://openweathermap.org/api/geocoding-api'),
                 ('Actual key activation, selected product access and remaining account allowance are '
                  'unverified; a newly generated key alone is not ready access.',)),
 'oxylabs': (('developer_registration',
              'Reviewed 2026-10-03. Create/select an Oxylabs account, activate Web Scraper API through its '
              "plan or trial, and create/select an API user in the dashboard. The API user's username and "
              'password authenticate the realtime endpoint with Basic authentication. Configure username and '
              'store that API-user password through api_key_env; despite the field name, it is not a bearer '
              'API key. Use the dedicated API-user credential, not an assumed website-login password.'),
             ('account',
              'search_google submits source=google_search and get_amazon_product submits '
              'source=amazon_product, both with parse=true to the fixed realtime Web Scraper API endpoint. '
              'The account must have the applicable target/product entitlement and sufficient quota or '
              'credit. These are paid/usage-metered scrape requests even though the connector retrieves '
              'data. This credential does not provide a per-selected-operation OAuth scope and is not '
              "interchangeable with a proxy-zone username/password or another Oxylabs product's key."),
             ('developer_registration',
              'Provider setup references checked 2026-10-03: '
              'https://developers.oxylabs.io/help-center/getting-started/start-using-web-scraper-api ; '
              'https://developers.oxylabs.io/help-center/most-popular-questions/getting-started-web-scraper-api '
              '; '
              'https://developers.oxylabs.io/help-center/dashboard/web-scraper-api-101-navigating-the-dashboard'),
             ("The API user's active state, selected product/target entitlement, account quota/credit and "
              'current usage restrictions remain account-owner facts.',)),
 'plivo': (('developer_registration',
            'Reviewed 2026-10-03. Select the intended Plivo account in Console and obtain its Auth ID and '
            'Auth Token from the dashboard through an authorized operator. For isolation, use an existing '
            "subaccount's own credentials; each subaccount has a separate Auth ID and token. Store the token "
            'through host custody and configure the matching auth_id. This is HTTP Basic authentication, not '
            'a generic bearer API key or an OAuth login.'),
           ('account',
            'get_call_details reads an existing call detail record by call UUID under that account. It does '
            'not call the separate live-call endpoint, create calls, send messages or provision numbers. The '
            'Auth Token is an account credential, not a selectable read-only call scope. The requested UUID '
            'must belong to the chosen account/subaccount; outbound geo-permission and caller-verification '
            'setup should not be imposed merely to read an existing record.'),
           ('developer_registration',
            'Provider setup references checked 2026-10-03: https://www.plivo.com/docs/voice/api/overview ; '
            'https://www.plivo.com/docs/voice/api/calls ; '
            'https://docs.plivo.com/docs/account/concepts/subaccounts'),
           ('The intended account/subaccount, authorized credential operator, credential validity and record '
            'ownership/retention remain account-owner facts.',)),
 'reddit': (('developer_registration',
             'Request and obtain explicit Reddit Data API approval before use. The current Responsible '
             'Builder Policy directs developers to Devvit; if the intended external application is not '
             'supported there, use its linked support request and describe the actual use case. Commercial '
             'use requires explicit written approval. Only after Reddit authorizes the external client '
             'should the host register/configure the permitted OAuth app at reddit.com/prefs/apps: web for a '
             'confidential hosted client, installed for a public client, or script for a controlled '
             'personal-account client. Those legacy app types are technical choices, not current eligibility '
             'guarantees. The host supplies the resulting bearer through api_key_env and a truthful '
             'descriptive user_agent; it does not send client secret or user password to this adapter.'),
            ('account',
             'get_top_posts reads /r/{subreddit}/top; get_subreddit_stats reads /r/{subreddit}/about; '
             "get_user_info reads /user/{username}/about. Reddit's endpoint reference assigns read to all "
             'three, so request read for any selected subset. get_user_info is public-user information, not '
             '/api/v1/me and does not need identity. No history, submit, vote, moderator or private-message '
             'scope is needed. Scope does not override private/quarantined community restrictions, account '
             'restrictions or the approved use case. Use a User-Agent of the documented '
             'platform:app-id:version (by /u/contact) form. OAuth acquisition/refresh stays with the host, '
             'and no scraping or alternate endpoint fallback is supplied.'),
            ('developer_registration',
             'Provider setup references checked 2026-10-03: '
             'https://support.reddithelp.com/hc/en-us/articles/42728983564564-Responsible-Builder-Policy ; '
             'https://support.reddithelp.com/hc/en-us/articles/16160319875092-Reddit-Data-API-Wiki ; '
             'https://github.com/reddit-archive/reddit/wiki/OAuth2 ; https://www.reddit.com/dev/api/'),
            ("Reddit's explicit approval for this external client and intended use, permitted app type, "
             'commercial status and assigned quota remain unverified.',
             'Token validity/read grant, target subreddit or user visibility and compliance with the '
             'approved data-retention conditions require the account/application owner to establish.')),
 'salesforce': (('developer_registration',
                 'Have the Salesforce administrator create an External Client App from Setup > App Manager > '
                 'New External Client App (or External Client App Manager), enable OAuth and select a flow '
                 'appropriate to the host. Register the host-owned callback only when the selected flow '
                 'needs one; do not copy a Salesforce CLI or Mobile SDK example callback. New Connected App '
                 "creation is restricted from Spring '26; existing Connected Apps may continue, and a new "
                 'exception requires Salesforce Support. For admin-preauthorized use, assign the app to the '
                 'intended profile or permission set. The host acquires an API access token for the chosen '
                 "org and supplies it through api_key_env with that org's instance_url. No password login, "
                 'token acquisition or refresh is performed by this adapter.'),
                ('account',
                 'get_record performs one REST sObject Rows GET by object name and record ID. Enable OAuth '
                 "scope api (Manage user data via APIs) and give the token's user API Enabled plus Read on "
                 'the selected object, visibility of the needed fields and access to the particular record '
                 'through sharing. The api scope itself is not a read-only provider grant; '
                 'object/field/record permissions constrain it. Do not grant Modify All Data, object write '
                 'permissions, web or full solely for this read. With no fields query, Salesforce returns '
                 'accessible record fields, not hidden fields. refresh_token/offline_access is a host '
                 'renewal decision, not an operation requirement. The org edition must support API access.'),
                ('developer_registration',
                 'Provider setup references checked 2026-10-03: '
                 'https://developer.salesforce.com/docs/platform/api-rest/guide/intro-oauth-and-connected-apps.html '
                 '; https://developer.salesforce.com/docs/platform/mobile-sdk/guide/eca-create.html ; '
                 'https://developer.salesforce.com/docs/platform/sfdx-dev/guide/sfdx-dev-auth-eca.html ; '
                 'https://developer.salesforce.com/docs/platform/salesforce-developer-environment-tipsheet/guide/salesforce-developer-environment-overview.html '
                 '; https://developer.salesforce.com/docs/platform/api/guide/calls.html ; '
                 'https://developer.salesforce.com/docs/platform/api-rest/guide/dome-get-field-values.html ; '
                 'https://developer.salesforce.com/docs/data/data-cloud-dev/guide/dc-quick-start.html'),
                ("Org API entitlement, the user's license/API Enabled permission, External Client App "
                 'policy/approval and token validity are not verified.',
                 'The selected instance_url, API version, object/record identity, field visibility, sharing '
                 'and any org IP/session restrictions require the org owner to confirm.')),
 'shopify': (('developer_registration',
              "Reviewed 2026-10-03. Create a new app through Shopify's Dev Dashboard, configure its Admin "
              'API scopes, release the app version and install it on the intended store using an authorized '
              'store operator. For an app and store owned by the same organization, the host can use the '
              'documented client-credentials grant to obtain the Admin access token; that token lasts 24 '
              'hours and must be replaced by its owner. For other merchants, use the appropriate Shopify app '
              'authorization flow. New legacy admin-created custom apps are unavailable from January 1, '
              '2026, although existing legacy apps remain a separate case. Supply the resulting token, never '
              "the app's client secret, to api_key_env."),
             ('account',
              'get_products needs read_products, including the selected variants and inventoryQuantity '
              'fields; this query does not read inventory levels by location or request inventory writes. '
              'get_shop_info reads basic shop/contact/address/plan fields via an authenticated Admin API '
              'token; do not invent a read_shop scope or require product access when only that operation is '
              'selected. The token must belong to shop_name, and api_version must support the exact selected '
              'GraphQL fields. The retained query uses the currently deprecated billingAddress field. A '
              'token generated for another shop or the Storefront API is not interchangeable.'),
             ('developer_registration',
              'Provider setup references checked 2026-10-03: '
              'https://shopify.dev/docs/apps/build/authentication-authorization/client-credentials-grant ; '
              'https://help.shopify.com/en/manual/apps/about-apps ; '
              'https://help.shopify.com/en/manual/apps/managing-apps ; '
              'https://shopify.dev/docs/api/admin-graphql/latest/objects/ProductVariant ; '
              'https://shopify.dev/docs/api/admin-graphql/latest/objects/Shop'),
             ("The organization's app/store ownership, staff installation/development permissions, approved "
              'scopes, selected supported API version and current token expiry remain shop/application-owner '
              'facts.',)),
 'spotify': (('developer_registration',
              'Create a Spotify developer app in the Developer Dashboard and configure the redirect URI '
              "actually owned by the host's authorization flow. For user data, the host uses Authorization "
              'Code with a safely stored client secret, or Authorization Code with PKCE for a public client; '
              'this adapter only receives the resulting access token through api_key_env. Current '
              "Development Mode requires the app owner's active Premium subscription, limits newly created "
              'access to one client ID and five authorized users, and requires users in Dashboard Settings > '
              'Users Management. Existing excess clients/users are grandfathered; Extended Quota Mode has '
              'separate approval. Do not infer usable access from successful consent alone.'),
             ('account',
              'Select scopes by operation: get_my_top_tracks requires user-top-read. get_user_playlists '
              "calls /me/playlists for the token's user, not an arbitrary user ID; playlist-read-private "
              'covers private playlists and playlist-read-collaborative includes collaborative playlists. '
              'get_current_user calls /me: user-read-email governs email and user-read-private governs '
              'private subscription/country fields where those fields are available; these are not blanket '
              'scopes for all four operations. Development Mode no longer returns email, country, product or '
              'explicit_content, so requesting these scopes does not restore those fields. search_tracks '
              'calls catalog /search; its endpoint reference lists no user scope, while the general scope '
              'index also associates Search with user-read-private. A search-only app-token route therefore '
              'remains a documented-source discrepancy, not verified account access. Development Mode search '
              'now permits at most 10 results per call; this MCP profile enforces that bound before dispatch. '
              'Playlist counts accept current items and legacy tracks fields; unavailable counts are null. No '
              'playback or write scope is required by these selected reads.'),
             ('developer_registration',
              'Provider setup references checked 2026-10-03: '
              'https://developer.spotify.com/documentation/web-api/concepts/apps ; '
              'https://developer.spotify.com/documentation/web-api/concepts/authorization ; '
              'https://developer.spotify.com/documentation/web-api/concepts/quota-modes ; '
              'https://developer.spotify.com/documentation/web-api/tutorials/february-2026-migration-guide ; '
              'https://developer.spotify.com/documentation/web-api/reference/get-current-users-profile ; '
              'https://developer.spotify.com/documentation/web-api/reference/get-users-top-artists-and-tracks '
              '; '
              'https://developer.spotify.com/documentation/web-api/reference/get-a-list-of-current-users-playlists '
              '; https://developer.spotify.com/documentation/web-api/reference/search ; '
              'https://developer.spotify.com/documentation/web-api/concepts/scopes'),
             ("The selected app's quota mode, owner Premium status, authorized-user list, granted token "
              'scopes and token expiry have not been observed.',
              'The application owner must establish that the intended use of Spotify content is permitted, '
              "including Spotify's restrictions on content use with AI; this help does not establish "
              'approval or an exception.')),
 'tavily': (('developer_registration',
             'Create or sign in to the selected Tavily account at app.tavily.com and obtain its dashboard '
             'API key. Pass it to the host credential owner for api_key_env; do not paste it into '
             'configuration or chat. This pinned profile requires a key even where newer Tavily products '
             'offer keyless access.'),
            ('account',
             "web_search_using_tavily uses the account's Search API allowance. Confirm available credits and "
             'request limits for the fixed basic search depth. This key-based route does not request OAuth '
             'scopes or register a callback.'),
            ('developer_registration',
             'Provider setup references checked 2026-10-03: https://docs.tavily.com/documentation/quickstart '
             '; https://docs.tavily.com/documentation/api-reference/endpoint/search'),
            ("The selected Tavily key's account, active status, remaining Search credits and applicable "
             'limits are not established by this help.',)),
 'todoist': (('developer_registration',
              'For your own account, obtain the personal API token under Todoist Settings, Integrations, '
              'Developer. For other users, the application operator registers an app and redirect URL in '
              "Todoist's App Management Console and runs user OAuth consent outside this adapter. Supply "
              'only the resulting API/access token through api_key_env; retain any client secret and refresh '
              'token with the host credential owner.'),
             ('account',
              'get_task reads one active task at /api/v1/tasks/{task_id}. A user OAuth grant can use '
              'data:read; this operation does not require data:read_write or task:add. A personal token is '
              'broader than this one-operation adapter. Newly created OAuth applications issue expiring '
              'tokens and refresh tokens, so the host must own renewal; the adapter only consumes the '
              'supplied access token.'),
             ('developer_registration',
              'Provider setup references checked 2026-10-03: https://developer.todoist.com/ ; '
              'https://developer.todoist.com/api/v1/#tag/Authorization ; '
              'https://developer.todoist.com/api/v1/#tag/Tasks/operation/get_task_api_v1_tasks__task_id__get'),
             ('Selected-user task/project visibility, actual granted scope and token validity are '
              'unverified. If OAuth is used, application approval, callback handling and renewal must be '
              'supplied by the host; no managed Todoist application is included.',)),
 'trello': (('developer_registration',
             "Reviewed 2026-10-03. Create or select a Power-Up in Trello's Power-Up admin portal and "
             'generate its API key on the API Key tab. Have the intended Trello user authorize that key with '
             "scope=read and an explicit expiry through Trello's documented /1/authorize flow. The host "
             'credential owner stores the resulting user token separately from the API key. api_key_env and '
             'token_env name that pair; this route does not use an OAuth app secret or construct OAuth '
             'signatures.'),
            ('account',
             'list_boards reads /1/members/me/boards and needs read, not write or account (email) scope. '
             'Results are boards visible to the authorizing user, including their applicable Workspace/board '
             'membership. The filter parameter chooses returned boards; it is not a provider authorization '
             'boundary. read grants broader Trello read access than this one operation, while the adapter '
             'exposes no board mutations. The selected API key and user token must belong to the same '
             'authorization.'),
            ('developer_registration',
             'Provider setup references checked 2026-10-03: '
             'https://developer.atlassian.com/cloud/trello/guides/rest-api/authorization/ ; '
             'https://developer.atlassian.com/cloud/trello/rest/api-group-members/'),
            ("The intended user's Workspace/board visibility, organization Power-Up restrictions and chosen "
             'token expiry/revocation state remain account-owner facts.',)),
 'twilio': (('developer_registration',
             'Reviewed 2026-10-03. Select the intended Twilio account or subaccount in Console and obtain '
             'its Account SID and Auth Token from Account Info through an authorized operator. Put only the '
             'Auth Token in the host credential store and use the matching account_sid in configuration. '
             'Twilio recommends API keys for production, but an API-key SID/secret is a different credential '
             'pair and is not implemented by this selected Account-SID/Auth-Token route. This component does '
             'not create a Twilio account or perform credential rotation.'),
            ('account',
             'get_call_details only fetches a known Call SID under the configured account. The account Auth '
             'Token has broad account authority rather than a read-only Call permission, so use an '
             'appropriately isolated account/subaccount when needed. No number purchase, messaging '
             'permission, outbound caller verification or call-creation grant is required by this read '
             'operation itself. The fixed api.twilio.com endpoint and explicit account pair do not implement '
             'a regional/edge credential selector or arbitrary parent-account/subaccount credential mix.'),
            ('developer_registration',
             'Provider setup references checked 2026-10-03: '
             'https://help.twilio.com/articles/223136027-Auth-Tokens-and-How-to-Change-Them ; '
             'https://www.twilio.com/docs/usage/requests-to-twilio ; '
             'https://www.twilio.com/docs/voice/api/call-resource'),
            ('The chosen account/subaccount, operator permission to obtain its token, credential validity '
             'and whether the requested Call SID belongs to it remain account-owner facts.',)),
 'unsplash': (('developer_registration',
               'Register a developer account with Unsplash, open Your apps and create a New Application. '
               "Supply its Access Key through api_key_env, not its Secret Key or a user's password. New "
               'applications start in demo mode; request production approval when the intended usage needs '
               'it.'),
              ('account',
               'search_photos and get_photo use public authentication with Authorization: Client-ID and need '
               'no end-user OAuth scope. Demo applications have a 50-request hourly limit. Image display '
               "must follow Unsplash's attribution and hotlinking guidance; downloading or selecting an "
               'image for use may require download tracking, which these two operations do not implement. '
               'Decentralized installations need their own approved key arrangement, not a shared embedded '
               'developer key.'),
              ('developer_registration',
               'Provider setup references checked 2026-10-03: '
               'https://unsplash.com/documentation#creating-a-developer-account ; '
               'https://unsplash.com/documentation#public-authentication ; '
               'https://help.unsplash.com/en/articles/2511245-unsplash-api-guidelines'),
              ("Application approval, current rate allowance and the consuming application's attribution, "
               'hotlinking and any download-tracking implementation are not verified here.',)),
 'webex': (('developer_registration',
            'For user-authorized operation, sign in to Webex for Developers, open My Webex Apps > Create a '
            "New App > Create an Integration, register the host's real OAuth redirect URI and select only "
            'the required integration scope. Retain the client secret with the host credential owner; the '
            'host handles user consent, callback, exchange and refresh and supplies only the access token '
            'through api_key_env. A developer personal access token is suitable for short-lived exploration, '
            "not a durable production credential. An existing bot token represents the bot, not the user's "
            "account; do not expect it to list the user's spaces. No Webex token is created by setup help."),
           ('account',
            'list_rooms calls GET /v1/rooms and needs spark:rooms_read for an integration. It lists spaces '
            'the authenticated principal belongs to, not every space in the organization; ensure that user '
            'or bot has membership in the required spaces. Use the narrow room-read grant rather than '
            'substituting spark:all. No room write, message-read, organization admin or compliance scope is '
            'needed for this selected listing. The adapter requests at most 100 rooms and refuses a '
            'paginated response, so membership in many rooms is a coverage limitation, not a reason to '
            'expand privileges.'),
           ('developer_registration',
            'Provider setup references checked 2026-10-03: '
            'https://developer.webex.com/admin/docs/integrations ; '
            'https://developer.webex.com/admin/docs/integration-scopes ; '
            'https://developer.webex.com/meeting/docs/getting-your-personal-access-token ; '
            'https://developer.webex.com/messaging/docs/api/v1/rooms/list-rooms ; '
            'https://developer.webex.com/blog/announcing-the-official-webex-postman-workspace ; '
            'https://developer.webex.com/messaging/docs/bots'),
           ("The token principal, actual room memberships, token lifetime and organization's integration "
            'approval restrictions have not been observed.',
            "Whether the principal's visible room list fits the adapter's single-response limit is "
            'unverified; this help does not certify complete organization coverage.')),
 'x': (('developer_registration',
        'Obtain a developer account and create/select a Project and App in the X Developer Console. For '
        "these public reads, the host can use the app's Bearer Token from its Keys and Tokens/credentials "
        'page. Current self-service API access uses prepaid pay-per-usage credits; review endpoint pricing, '
        'available balance and spending limit before enabling requests. If acting for a user instead, the '
        'host configures OAuth 2.0 Authorization Code with PKCE and its actual registered callback, '
        'completes user consent and supplies that access token through api_key_env. API key/secret and OAuth '
        '1.0a access-token/secret pairs are not interchangeable with this single bearer input.'),
       ('account',
        'get_user_info calls GET /2/users/by/username/{username}; search_posts calls GET '
        '/2/tweets/search/recent. Both accept OAuth 2.0 app-only bearer access for public data. If using an '
        'OAuth 2.0 user-context bearer, the authentication mapping requires tweet.read and users.read for '
        'either selected operation; these are not scopes to attach to an app-only token. No tweet.write, '
        'follow, direct-message or email scope is needed. offline.access is relevant only if the host '
        'separately implements renewal. search_posts covers recent search (last seven days), not full '
        'archive. include_post_metrics only adds public_metrics, not private analytics. Billing and endpoint '
        'rate limits still apply to read-only requests.'),
       ('developer_registration',
        'Provider setup references checked 2026-10-03: https://docs.x.com/x-api/users/lookup/introduction ; '
        'https://docs.x.com/make-your-first-request ; https://docs.x.com/x-api/getting-started/pricing ; '
        'https://docs.x.com/fundamentals/authentication/overview ; '
        'https://docs.x.com/fundamentals/authentication/oauth-2-0/authorization-code ; '
        'https://docs.x.com/fundamentals/authentication/guides/v2-authentication-mapping ; '
        'https://docs.x.com/x-api/posts/search/introduction'),
       ('Developer/app approval, current endpoint entitlement, credit balance/spending limit, granted user '
        'scopes if applicable and token validity remain unverified.',
        'The pinned adapter still sends requests to api.twitter.com, while current documentation uses '
        'api.x.com. Reachability and compatibility of that retained origin for this account have not been '
        'tested; these records do not change the runtime origin.')),
 'zendesk': (('developer_registration',
              'Reviewed 2026-10-03. For OAuth, have the host credential owner supply an existing access '
              'token for the selected Zendesk instance through api_key_env and set '
              'auth_mode=oauth_access_token. Omit account_email. The host owns OAuth client registration, '
              'consent or client-credentials exchange, secure token storage and renewal; this connector '
              'does not acquire or refresh tokens. Reconstruct the profile with the renewed credential '
              'when the host replaces it. Tokens from authorization-code and client-credentials flows '
              'retain their provider-defined acting user; client-credentials calls act as the OAuth '
              'client creator. Multi-customer integrations require a provider-approved global OAuth '
              'client; accepting its access token does not register or approve that application. '
              'The legacy api_token mode is unavailable to Zendesk accounts created '
              'on or after July 28, 2026. For an older internal-use account still permitted to create a '
              'token before October 27, 2026, an administrator enables API-token access and uses Admin '
              'Center, Apps and integrations, APIs, API tokens, Add API token. Keep it in host custody and '
              'select a verified account_email; the connector sends email/token plus the token as Basic '
              'authentication. From July 28, unused tokens deactivate after 30 days and are deleted after 60 '
              'additional deactivated days. New token creation ends October 27, 2026; all tokens stop April '
              '30, 2027. Omitted auth_mode preserves this legacy route; it does not select OAuth '
              'automatically.'),
             ('account',
              'search_zendesk calls Help Center article search only, not Ticketing search. Article '
              "visibility follows the selected verified user's Help Center permissions and user segments; "
              "use the intended brand's subdomain for a multi-brand account. The selected Search Articles "
              'documentation shows Bearer authentication but does not state a per-endpoint OAuth scope '
              'requirement. The host/account owner must confirm the least required scope and actual '
              'article visibility; scope examples for Embeddable Widget Search describe a different '
              'endpoint. Configuring a token or subdomain '
              'does not verify its subject, issuer, client, scope or instance. An API token is account-wide '
              'and can authenticate as other verified users when paired with their email, so it is not '
              "intrinsically limited to the configured email or read-only article search. The connector's "
              "read restriction does not narrow the secret's provider authority."),
             ('developer_registration',
              'Provider setup references checked 2026-10-03: '
              'https://support.zendesk.com/hc/en-us/articles/4408889192858-Managing-API-token-access-to-the-Zendesk-API '
              '; https://support.zendesk.com/hc/en-us/articles/8662004958746-Turning-on-and-off-API-access ; '
              'https://developer.zendesk.com/api-reference/introduction/security-and-auth/ ; '
              'https://developer.zendesk.com/documentation/authentication/oauth-migration/ ; '
              'https://developer.zendesk.com/api-reference/help_center/help-center-api/help_center_search/ ; '
              'https://developer.zendesk.com/api-reference/help_center/help-center-api/introduction/ ; '
              'https://support.zendesk.com/hc/en-us/articles/10840968198042-Announcing-the-removal-of-API-tokens-as-an-authentication-method-for-API-requests'),
             ('OAuth registration, consent, token validity/renewal, acting user and actual scopes remain '
              'host/account-owner facts. The account creation date, continued permission to use legacy '
              "tokens, token activity/deactivation state, selected verified user's article visibility "
              'and correct brand subdomain also remain unverified.',)),
 'zoom': (('developer_registration',
           'In Zoom App Marketplace, open Develop/Created Apps and Build App. Use a General OAuth app with '
           "user-managed access for each consenting user's own meetings, or admin-managed access for "
           "authorized account-wide access. Configure the host's actual OAuth redirect URL and complete "
           'applicable account approval/distribution requirements. For unattended access solely within your '
           'own Zoom account, an authorized account developer can instead create and activate a '
           'Server-to-Server OAuth app with the selected scopes; that route uses account credentials rather '
           'than a user callback. The host obtains and renews the access token and supplies api_key_env. '
           'Client ID, client secret and account ID are not bearer tokens for this adapter.'),
          ('account',
           'For list_meetings, request granular meeting:read:list_meetings for user-level access, or '
           'meeting:read:list_meetings:admin for approved account-level access. User-level calls must use '
           'user_id=me; account access to another host requires the corresponding account authority. For '
           'get_meeting, request meeting:read:meeting or meeting:read:meeting:admin respectively. Do not '
           "request the other operation's granular scope when that operation is disabled. Existing classic "
           'meeting:read/meeting:read:admin grants are documented alternatives, but are broader. No meeting '
           'write, recording, webinar or master-account scope is needed. get_meeting requires a host with '
           'Zoom Meetings Basic or higher. list_meetings is the scheduled/unexpired-meeting endpoint and '
           'returns only the retained first-page view, not all historical or instant meetings.'),
          ('developer_registration',
           'Provider setup references checked 2026-10-03: '
           'https://developers.zoom.us/docs/integrations/create/ ; '
           'https://developers.zoom.us/docs/build-flow/basic-info/app-mgmt/ ; '
           'https://developers.zoom.us/docs/internal-apps/create/ ; '
           'https://developers.zoom.us/docs/integrations/oauth-scopes-granular/ ; '
           'https://developers.zoom.us/docs/api/meetings/'),
          ("The selected app's activation/approval, user-managed or account-managed authority, granted "
           'scopes, account developer permissions and access-token validity remain unverified.',
           'The requested meeting/host must belong to the access actually granted to this token; host '
           'license, account restrictions and current meeting visibility are not established by '
           'configuration help.'))}


def configuration_prerequisites(profile: str) -> tuple[list[dict[str, str]], list[str]]:
    """Project fresh inert rows after the caller validates the exact profile ID."""
    public = profile in _PUBLIC
    meaning = _PUBLIC[profile] if public else _CREDENTIALS[profile]
    rows = [{"kind": "application_binding", "instruction": _LOCAL_MCP},
            {"kind": "client_authentication", "instruction": meaning}]
    if not public:
        rows.append({"kind": "client_authentication", "instruction": _INJECTION})
    rows.extend({"kind": kind, "instruction": instruction}
                for kind, instruction in _CONTEXT.get(profile, ()))
    unresolved = [
        "This help describes the selected adapter and retained guide. It does not "
        "verify current provider availability, account identity, resource access, "
        "authorization or readiness."
    ]
    if profile in _PROVIDER_GUIDANCE:
        registration, permissions, references, account_unknowns = _PROVIDER_GUIDANCE[profile]
        rows.extend({"kind": kind, "instruction": instruction}
                    for kind, instruction in (registration, permissions, references))
        unresolved.extend(account_unknowns)
    elif profile == "postgres":
        unresolved.append("The database principal, SCRAM configuration and actual metadata visibility require the database owner's verification.")
    return rows, unresolved
