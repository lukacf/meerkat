# Third-party source in the Python SDK

`meerkat_toolkit/adapters/agno_transports.py` adapts request and client construction helpers from Agno revision `8c3d8ec52b4a13c410fca97d07fbd507790cf29a`, under `libs/agno/agno/tools/`:

| Source file | Exact source SHA-256 |
| --- | --- |
| `financial_datasets.py` | `1d57cfe55f095f94deba7828a1c09116aa110581e13e950fed7943c860f378f7` |
| `openweather.py` | `48879b103ec86de53697c2cbb495ab97007ac6b2b7917de5ce068559b62f9578` |
| `bitbucket.py` | `2550423ee08a557446cbeda5d3ae6da8c519da2b85ed2a6e9e66d601fa386b5b` |
| `gitlab.py` | `487b529743af8752eaede5c2034bcdf7c0d89ef2a0848b3154fd442bb9b661f7` |
| `clickup.py` | `91ae7bafe1791ab00e9d1bd6998f4140f9db250cb12bba73e83055965ac3fc07` |
| `discord.py` | `9fa239fb2e0c3b318b214c33a53b60c5876952b2668855fad6d3f27bcc022c9b` |
| `shopify.py` | `a5a7eefd2d9fb8923decf9c6eb52bc4d96773a1b5f4b4acf8820097e9e6623d0` |
| `spotify.py` | `9b75e3293ef63a450ae34288ce0561e99389edee6e0593c59e70d7a3d44e2fe8` |
| `unsplash.py` | `e4a89182cbaaa55f512002b047a8fe82e31393f30ac7d46fb2c807b08bc6f1c9` |
| `calcom.py` | `6cc4e8bd4d061a6d0ff14fd89ff825acb444812e6085d2f4613aad3ee22635e5` |
| `jina.py` | `ae8b552966500ecddf5fb4abfb75f05f50645361e92fb058afb5ff4473371883` |
| `openrouteservice.py` | `f2529e30b1996cb5f6965dd84f6d90bcbd14cc748fd882d84157fe5611f33014` |
| `zendesk.py` | `d90078c60a8efa4d21cabddd328ff6bf0558b173414bb3710eba2e30021da821` |
| `brightdata.py` | `b9a0fe8b6558fa8fb186b63d34f1c11186afe7df8822da420f332a5e9044fd83` |
| `linkup.py` | `5e276d832f9ca8f9a5d469f22cd445a1a815aa8420c7409da2e5dbdefceaac77` |
| `todoist.py` | `62560033c0ef1a8db07b01e4946a12daec2ac01ce101f5cfbf50d7fb05711325` |
| `zoom.py` | `b931845fd52a5d6fd7abc8275e9f51d1a67f11a17c252586873df92a45573e98` |
| `exa.py` | `593fc82a232ea3aa2815750ee6191c8c35de46ca43a51324563117e49aaf3df1` |
| `firecrawl.py` | `0361a79c24e488be3fda2c5824afc62a15b1061d820b6e999bba74b4f1e8f59a` |
| `trello.py` | `a8bddaaf8a824c24348daf7941588b406062b725c8e7f4baf71d7ec0a69b5cd4` |
| `salesforce.py` | `fefb9c5c4997a4d10725f49e8c898b73ce043c2b55c1100eb47e027ac6730371` |
| `twilio.py` | `7b00c4b451968a114864e74b1b0accbeb3355f72cfe635c66f3cfe2b4e66655d` |
| `webex.py` | `e4d2467f52181d7fcb98fa97660afc02bdcb82d2c30629378bccecd3063b4daf` |
| `plivo.py` | `6e157636f32a7dc04858d1e13f629bc0b661d2167f286eb99690271df19f0705` |
| `arxiv.py` | `b667b57fdfaec93c2a91d09c9d4e46451fc7d0b105b3b6c3da3c087b3af04c5d` |

The earlier request-helper adaptations inherit their selected operation bodies from the separately installed Agno distribution. The four sources added below use explicit operation ports because their HTTP calls occur inside the operation bodies.

The five service helpers use one shared bounded Requests JSON transport in place of the donors' Requests, HTTPX or urllib helpers. Each response closes on success or failure; decoded input is capped at 1 MiB and GraphQL errors remain failures. Selected read methods and their service-specific result formatting remain inherited. No image rights, provider access or write coverage follows from these source licenses.

Cal.com (`get_available_slots`, `get_upcoming_bookings`), Jina (`read_url`, `search_query`), OpenRouteService (`geocode_location`, `get_directions`) and Zendesk (`search_zendesk`) are explicit source ports in that same module. They retain the actual Agno Toolkit classes and pure header/parsing/formatting helpers, with no production globals patching. Their HTTP calls use the same owned bounded Requests transport; raw error bodies and diagnostic logs are replaced with fixed errors. Cal.com adds real-date/31-day bounds and timezone-aware 1970-2037 timestamp checks before the inherited `pytz` formatter. Jina retains JSON parsing, Python string conversion and the original truncation suffix while enforcing a real socket timeout; plaintext provider responses remain errors. OpenRouteService retains first-feature geocoding and route-summary formatting, rejects invalid coordinates and absent, boolean, nonfinite or negative route metrics, and makes at most three requests. Zendesk retains regex HTML removal and JSON serialization, with explicit selected-site API-token Basic authentication. Cal.com per-date slots and Zendesk article results must be actual lists; valid empty collections retain donor formatting. The 2026-10-04 Cal.com compatibility candidate additionally ports the documented /v2/slots range response (2024-09-04) and /v2/bookings cursor metadata (2026-05-01). It keeps one booking page of at most 100, validates the response shape and marks nonterminal results incomplete without following the cursor. These Cal.com changes retain the same donor attribution and dependencies and await component review and promotion. Writes, route matrices, tickets and frontends are not ported. The metadata adds the exact `pytz==2026.4` dependency; its installed MIT notice remains with that distribution.

Agno's copyright and Apache-2.0 license text are retained verbatim in `LICENSE-Agno`. The adapted helpers borrow explicit Requests sessions, refuse redirects and omit raw exception diagnostics. The Bitbucket helper selects the first pull-request page; the GitLab client disables SDK retries. Constructors receive selected credentials and sessions; the Toolkit profile owns each session's lifecycle. The module retains the upstream notice and identifies these changes.

Other Agno code and its optional provider SDKs are declared package dependencies, not embedded copies. Their installed distributions retain their own notices. The `agno-mcp` extra pins the selected runtime versions; its installed-artifact verification identifies the tested dependency graph.

The nine PairedHost-derived module copies listed in `LICENSE-EXTRACTED` are licensed under MIT OR Apache-2.0. Their source provenance is retained in that notice. This grant applies only to those extracted Toolkit copies; the original paired host repositories and separately distributed dependencies retain their own terms. The package license expression also preserves the existing Apache-2.0, BSD-2-Clause and MIT donor obligations.


Bright Data inherits `scrape_as_markdown` and `search_engine`. Its selected request helper borrows the shared byte-bounded transport, requires HTTP 200, decodes strict UTF-8 and wraps successful Markdown as `{content: text}` so page prose cannot be mistaken for donor error text. Explicit configured zones replace the donor's ambient override. Screenshots and dataset jobs are excluded.

Linkup inherits `web_search_with_linkup` and uses the real `linkup-sdk==0.23.1` search method/Pydantic models. The SDK request helper uses the owned transport and fixes endpoint/status; an SDK method wrapper converts the actual model to plain JSON and suppresses raw parsing diagnostics. The SDK's original Pydantic return value cannot be passed unchanged through the plain-JSON adapter. This is an explicit serialization adaptation.

Todoist ports only construction to inject the real `todoist-api-python==4.0.0` public HTTPX client seam, registering the original `get_task` operation and `_task_to_dict` helper. The SDK's real method/model uses a bounded transport bridge over the same Requests session, with request authority and response identity/type checks. An SDK method wrapper sanitizes parsing errors before donor logging. HTTPX owns no additional socket transport. The profile retires its SDK client after worker joining; the shared session remains profile-owned. Other Todoist operations are not exposed.

Zoom explicitly ports construction and `list_meetings`/`get_meeting` for an already selected bearer token. The ports preserve JSON-string results and selected field projection, reject malformed collections and identities, and retain absence of optional fields rather than manufacturing empty/zero values. Module-level HTTP, OAuth acquisition/refresh, raw diagnostics, recordings and writes are excluded. All four profiles use the existing serialized/joined MCP lifecycle without modifying it.

The three added dependency artifacts are `linkup-sdk==0.23.1` (MIT), `todoist-api-python==4.0.0` (MIT) and `dataclass-wizard==0.39.1` (Apache-2.0). Their installed distributions retain their original notices; their SDK source is not vendored into this component. The existing 60 runtime constraints remain unchanged. Source review records exact wheels, license hashes and the added three-pin closure; fresh installed acceptance is separate work.


Exa, Firecrawl, Salesforce, Webex, Plivo and arXiv retain their selected Agno operation bodies and actual SDK methods/models. Exa replaces the real SDK request method and sanitizes parser errors before donor logging. Firecrawl replaces its real v2 HTTP client on that instance; its Document/map models and parameter preparation remain in use. The shared transport borrows one owned Requests session, validates the selected origin, route, credential binding and consumed provider fields, requires an actual HTTP 200, bounds response bytes before decoding, and never follows redirects or retries. Returned Requests responses contain the provider's observed successful bytes and headers; a failed provider response is never converted into success.

Trello, Salesforce, Twilio, Webex, Plivo and arXiv use explicit constructor ports. Trello injects the real TrelloClient public `http_service` seam; its non-OAuth `api_secret` slot holds the selected user token and its actual SDK emits the checked `key`/`token` pair. The original Board.from_json drops starred/idOrganization and defaults absent desc, while the donor invents false/null metadata. A narrow operation port retains actual TrelloClient.fetch_json and Board.from_json execution, projects required fields from the real Board, and includes description/starred/organization only when the validated provider payload includes the corresponding field. Explicit null, false and empty string remain unchanged; absent optional keys remain absent. No fake Board or global model patch is used. Salesforce injects the real client's `session` with an existing session token and selected instance. Only the inherited complete-record branch is exposed; SOQL and password login are excluded. Its response identity check accepts the corresponding 18-character response to a selected 15-character ID. Twilio injects its public HttpClient and fixes account and origin. The original donor's `call.from_` fails against Twilio 9.11.2, whose real CallInstance exposes `_from`; an explicit operation port changes that lookup, retaining actual `calls(...).fetch()` and the donor's eight-field projection. It omits raw donor logging and preserves the donor's string conversion of absent or null times. Present times must match the entire supported form Wdy, DD Mon YYYY HH:MM:SS +0000 or GMT, with fixed English tokens, two-digit day, four-digit year, single ASCII spaces, real calendar/time bounds and matching weekday. The actual SDK parser must reproduce that validated instant exactly; years below 0100 are rejected because the pinned parser silently rewrites them. The actual SDK model still runs afterward. Prefix parsing and stripping are not used. Leading/trailing whitespace, line folding, comments, extra content, empty/malformed strings and unknown/non-UTC zones fail, rather than becoming None, losing trailing data or falsely relabeled UTC values. Other RFC 2822 representations are outside this profile. No SDK model or provider response is fabricated.

Webex closes the SDK-created Requests session and replaces that instance's `_session._req_session` with the borrowed transport. The real RoomsAPI/Room enumeration executes with `max=100`; a nonempty Link header or more than 100 entries fails explicitly, never yielding a partial all-rooms success. Plivo replaces the real RestClient `send_request` boundary after its genuine request preparation and before regional retry/fallback. The selected call UUID must match both path and the SDK's duplicate query field, and the real Call model remains in use. Its two otherwise unused SDK sessions are registered for retirement. arXiv closes/replaces the real client's instance `_session`, preserves its Atom parser/Result models and donor result formatting, and omits the original constructor's PDF-directory setup. Strict XML/feed checks reject malformed or silently skipped entries; an explicit empty feed remains valid. The SDK clients own no second active socket route. Profile retirement stays with the existing joined MCP lifecycle.

The exact direct additions are `exa-py==2.25.0`, `firecrawl-py==4.46.2`, `py-trello==0.20.1`, `simple-salesforce==1.12.10`, `twilio==9.11.2`, `webexpythonsdk==2.0.6`, `plivo==4.63.0`, `arxiv==4.0.1` and `pypdf==6.19.0`. pypdf is required by the pinned Agno arXiv import only; no PDF operation is exposed. The optional library closure preserves all prior 63 versions and adds 33 exact pins, for 96 total. Its source audit checks 3,807 non-RECORD installed payloads and 43 retained notice files against the 33 original wheels and records per-profile declared dependency closures. Dependency licenses include MIT, BSD-2-Clause, BSD-3-Clause, Apache-2.0, PSF-2.0, ISC and dual-license choices; each distribution's original metadata/notices remain authoritative. Provider SDK source is installed from those distributions, not vendored into this component or relicensed as Toolkit. These dependencies are not added to core Meerkat/MobKit. Source tests and dependency auditing do not establish successor installed-artifact acceptance, OAuth acquisition, live service readiness or write coverage.


## Six HTTP read profile source attributions

The following selected Agno 3.0.11 donor modules are retained exactly in the
installed Agno distribution, at commit `8c3d8ec52b4a13c410fca97d07fbd507790cf29a`:

| Source under `libs/agno/agno/tools/` | Original SHA-256 |
| --- | --- |
| `reddit.py` | `60ddb69ea8cb18c09f8a8633430ee3495482c5b2df24f82a20d604dd9c4880ea` |
| `x.py` | `8a5ee602606c31197162a1b07ec7f536150946b3d8fa78de8b603f7420551c1e` |
| `youtube.py` | `292519954dcb91d5c6b2459e6f1cde55401cc1f0bbaaf07af946706049dcef09` |
| `oxylabs.py` | `ff721fd8703a63352935625bf9675222403f4fd101da89f7cf46ca01ebdca3a0` |
| `wikipedia.py` | `f243f9022e6325bc42218b765657eaab3a6ae228b45805464dc958f9178b9a0e` |
| `aws_lambda.py` | `6710f0bd41683e439e5945499fb785f2da33100877615d4230b9f6faaeeac3db` |

Reddit retains the three actual Agno operation bodies and real PRAW 8.0.3
Subreddit/Redditor/Submission/ListingGenerator/objectification behavior.
Its PRAW constructor and request entry are explicitly ported: packaged SDK
configuration defaults only, selected existing bearer token and User-Agent,
no home/cwd/environment account discovery, authentication, update check,
retry or rate-limit sleep. Provider fields are validated before real model
construction, including response identity, collection/number/bool types and
bounds. `[deleted]` retains the real SDK/donor `None` string projection;
missing/null author fields are outside this selected response contract.
No claimed allowed-subreddit authorization or writes are introduced.

X retains real Tweepy 4.17.0 Client parameter preparation, Response/User/Tweet
models and original Agno formatting. The SDK request entry is a narrow port
using only the selected bearer and bounded transport, without raw-header
logging or rate-limit retries. The constructor excludes ambient OAuth1 fields.
Consumed user, tweet and author IDs are checked as canonical positive decimal
strings before Tweepy integer normalization. Search adds the author expansion
that the donor's formatter needs and wraps only its error projection into the existing single-key convention; successful
results still come from the original donor body. The SDK-created selected
client session closes immediately. Tweepy also allocates a process-global
legacy API session at import; this component does not borrow, modify, close or
perform I/O through that unrelated idle global. This does not establish OAuth
acquisition, OAuth1 provider-call or authorization-server flow support.

YouTube ports the pinned `get_youtube_video_id` and `get_youtube_video_data`
logic explicitly, preserving the fixed oEmbed operation and ten-field result
projection. Complete HTTPS authority/path/query and video-ID checks, actual
HTTP 200, response bounds/types and generic errors replace the original
module-level unbounded urllib call and permissive URL parsing. There is no
transcript dependency, captions, download or browser operation.

Oxylabs retains both original Agno operation bodies and actual SDK 3.0.0
Google/Amazon helper methods and Response models. Only each selected helper's
API instance is replaced by a real RealtimeAPI subclass using the shared
bounded transport and original credential headers. Its error helper is
narrowed to the existing MCP convention. Explicit result/status/parsed-field
validation prevents SDK empty defaults from disguising malformed or failed
jobs. Supplied job metadata must have the pinned SDK's terminal `done` status;
absent job metadata and valid explicit empty lists remain valid. One provider result and at
most 50 organic results are admitted. The Google validator follows the donor's
empty-parsed-list fallback selection, validating consumed raw results and
ignoring unused raw content after nonempty parsed results. No general scrape
or async job API is exposed.

Wikipedia explicitly ports Agno's `search_wikipedia` Document projection and
wikipedia 1.4.0's `summary`, page-loading and disambiguation request/parser
logic. Actual Agno Document and BeautifulSoup remain in use. HTTPS, an explicit
language, bounded redirects/options, strict page/revision shape checks and
generic errors replace the original module globals, cache and unbounded
recursion. Secondary summaries/revisions must retain the resolved page-map ID
and canonical title; explicit pageid fields must agree, and explicit
continuation markers fail. Initial title normalization remains supported.
Auto-suggestion and knowledge ingestion are excluded. The original
MIT-licensed wikipedia helper source and license are retained in the source
audit; the global wikipedia client is not a runtime dependency.

AWS Lambda ports construction to create a real botocore 1.43.108 client with
explicit credentials/region and no ambient configuration, credential provider,
metadata lookup or endpoint override. The original Agno list operation remains.
Its SDK endpoint entry is an explicit port that passes actual service
serialization through bounded owned HTTP and the actual protocol parser.
The narrow SigV4Auth.add_auth port omits canonical credential/session-token
logging but uses the actual SDK canonical request, string-to-sign, HMAC and
signature injection methods. Tests compare final signatures with the original
signer. The inherited call selects MaxItems=50; a continuation marker fails
instead of representing an incomplete enumeration as all functions. No
invocation, credential refresh, GovCloud/China/FIPS or custom endpoint support.
The SDK's unused connection pool retires with the profile; no second active
network route or runtime owner is introduced.

The successor source graph has 109 exact packages: 95 Agno29 versions remain,
OAuthlib moves from 4.0.0 to 3.3.1 to satisfy Tweepy >=3.2,<4, and 13 packages
are added. Both OAuthlib versions retain identical BSD-3-Clause license bytes.
The accepted Agno29 artifact/96-pin graph remains immutable. Exact wheel
payloads, per-profile closures, notices and the JMESPath installed-script
shebang projection are recorded in the source dependency audit. Dependency
source stays in its original installed distributions unless an attributable
entry/operation port is explicitly identified above. These optional SDKs do
not become Meerkat/MobKit core dependencies. Independent review and fresh
installed-artifact proof are still required for this six-profile successor.

### PRAW 8.0.3 source-port notice (BSD-2-Clause)

Copyright (c) 2016, Bryce Boe
All rights reserved.

Redistribution and use in source and binary forms, with or without
modification, are permitted provided that the following conditions are met:

1. Redistributions of source code must retain the above copyright notice, this
   list of conditions and the following disclaimer.
2. Redistributions in binary form must reproduce the above copyright notice,
   this list of conditions and the following disclaimer in the documentation
   and/or other materials provided with the distribution.

THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS "AS IS" AND
ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT LIMITED TO, THE IMPLIED
WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR PURPOSE ARE
DISCLAIMED. IN NO EVENT SHALL THE COPYRIGHT HOLDER OR CONTRIBUTORS BE LIABLE
FOR ANY DIRECT, INDIRECT, INCIDENTAL, SPECIAL, EXEMPLARY, OR CONSEQUENTIAL
DAMAGES (INCLUDING, BUT NOT LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS OR
SERVICES; LOSS OF USE, DATA, OR PROFITS; OR BUSINESS INTERRUPTION) HOWEVER
CAUSED AND ON ANY THEORY OF LIABILITY, WHETHER IN CONTRACT, STRICT LIABILITY,
OR TORT (INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT OF THE USE
OF THIS SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGE.


### Tweepy 4.17.0 source-port notice (MIT)

MIT License
Copyright (c) 2009-2023 Joshua Roesslein

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in
all copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN
THE SOFTWARE.


### wikipedia 1.4.0 helper-port notice (MIT)

Copyright 2013 Jonathan Goldsmith

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in
all copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN
THE SOFTWARE.

### Botocore 1.43.108 source-port notice (Apache-2.0)

The Apache-2.0 license text is retained in LICENSE-APACHE. Original NOTICE:

Botocore
Copyright 2012-2022 Amazon.com, Inc. or its affiliates. All Rights Reserved.

----

Botocore includes vendorized parts of the requests python library for backwards compatibility.

Requests License
================

Copyright 2013 Kenneth Reitz

   Licensed under the Apache License, Version 2.0 (the "License");
   you may not use this file except in compliance with the License.
   You may obtain a copy of the License at

       http://www.apache.org/licenses/LICENSE-2.0

   Unless required by applicable law or agreed to in writing, software
   distributed under the License is distributed on an "AS IS" BASIS,
   WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
   See the License for the specific language governing permissions and
   limitations under the License.

Botocore includes vendorized parts of the urllib3 library for backwards compatibility.

Urllib3 License
===============

This is the MIT license: http://www.opensource.org/licenses/mit-license.php

Copyright 2008-2011 Andrey Petrov and contributors (see CONTRIBUTORS.txt),
Modifications copyright 2012 Kenneth Reitz.

Permission is hereby granted, free of charge, to any person obtaining a copy of this
software and associated documentation files (the "Software"), to deal in the Software
without restriction, including without limitation the rights to use, copy, modify, merge,
publish, distribute, sublicense, and/or sell copies of the Software, and to permit persons
to whom the Software is furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all copies or
substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR IMPLIED,
INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A PARTICULAR
PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE
FOR ANY CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR
OTHERWISE, ARISING FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER
DEALINGS IN THE SOFTWARE.

Bundle of CA Root Certificates
==============================

***** BEGIN LICENSE BLOCK *****
This Source Code Form is subject to the terms of the
Mozilla Public License, v. 2.0. If a copy of the MPL
was not distributed with this file, You can obtain
one at http://mozilla.org/MPL/2.0/.

***** END LICENSE BLOCK *****


## PostgreSQL metadata profile source port and runtime dependencies

Pinned donor: Agno 3.0.11, revision `8c3d8ec52b4a13c410fca97d07fbd507790cf29a`,
`libs/agno/agno/tools/postgres.py`, SHA-256
`94be4e1ed53fe16b5f56ded1c023f6af9ab77bdf852730ff97a87d3dfa380b62`.
Agno is Apache-2.0; its full license is retained in `LICENSE-Agno`.
The `show_tables` and `describe_table` bodies remain inherited unchanged. The
connection/close and `_execute_query` helper port changes connection selection to
explicit lazy local Unix-socket SCRAM, read-only transactions and selected timeouts;
replaces unbounded fetchall with a real server cursor and limit-plus-one failure;
uses `csv.writer` for metadata quoting; and raises sanitized failures after
rollback instead of logging and returning raw database exceptions as strings.
Only the two metadata reads are selected. No Agno agent runtime is used.

External runtime distributions, unmodified and separately installed:

- `psycopg==3.3.6`, LGPL-3.0-only, wheel SHA-256
  `a1db9f7148b06a28606767efaca51fa6f9398c5c0a3810519be69d7000bdb631`.
- `psycopg-binary==3.3.6`, LGPL-3.0-only, qualified CPython3.11 macOSarm64 wheel
  SHA-256 `f0535693ce476a722b718b002d5d2c27d47e71ca945276ac194409c98e74c492`.
  This client artifact includes libpq18.0.6. The disposable server fixture is
  PostgreSQL16.2, a separate test prerequisite, not a product dependency.

Copyright (C) 2020 The Psycopg Team. The distributions retain their upstream
`licenses/LICENSE.txt`, SHA-256
`e3a994d82e644b03a792a930f574002658412f62407f5fee083f2555c5f23118`.
Upstream project/source: https://github.com/psycopg/psycopg (release 3.3.6).
No Psycopg source is copied into the Toolkit wheel. Its external LGPL runtime
license is distinct from Toolkit's source-port license expression. The source
qualification retains exact artifacts, full upstream license text, 113 installed
SDK payload comparisons and the PostgreSQL profile's SDK dependency closure.
The successor's 111 runtime pins preserve all 109 accepted Agno35 versions.
These dependencies remain part of this optional standalone integration library;
core Meerkat and MobKit dependencies are unchanged.

## Custom extension artifact verification

`extension_artifacts.py` and `_extension_wheels.py` extract generic wheel,
installed RECORD and import-ownership checks from the private paired host candidate
recorded in `EXTENSIONS.md`. These extracted Toolkit copies are licensed under
MIT OR Apache-2.0 as stated in `LICENSE-EXTRACTED`. Household policy,
activation and runtime ownership are excluded.

The optional `extensions` extra depends on the separately installed `packaging`
distribution, declared `packaging>=25.0` consistently with the captured paired host
candidate. The current verification uses packaging 26.3, whose distribution
declares Apache-2.0 OR BSD-2-Clause. No packaging source is copied. Retained wheel
and installed dependency license material remains the responsibility of the
host's package distribution.
