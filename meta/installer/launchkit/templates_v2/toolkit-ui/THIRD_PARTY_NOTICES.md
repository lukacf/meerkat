# Third-party source and dependency notes

The Slack read operations are an original Toolkit port of its Python Slack
contract, using the package's existing Fetch and strict-JSON pattern. No Slack
SDK source is copied. The protocol references are Slack's official
[history](https://docs.slack.dev/reference/methods/conversations.history/) and
[replies](https://docs.slack.dev/reference/methods/conversations.replies/)
documentation, inspected on 2026-10-01. No runtime dependency was added.

The Home Assistant operations are an original Toolkit implementation of the
public REST protocol. No Home Assistant server implementation is distributed
inside this package. Protocol and outcome rules were checked against
[Home Assistant core 6cd7e61bad577dd450e26dca2680fd80770543b1](https://github.com/home-assistant/core/tree/6cd7e61bad577dd450e26dca2680fd80770543b1),
notably `homeassistant/components/api/__init__.py` and its API tests. Home
Assistant core is Apache-2.0. In particular, its service route shields the
service task from client disconnect. A home-automation host is an eventual Toolkit consumer,
not a separately copied runtime or authentication implementation.

Runtime dependency `jsonc-parser` is pinned to 3.3.1, maintained by Microsoft,
and MIT licensed. The reviewed source corresponds to
[release commit 3c9b4203d663061d87d4d34dd0004690aef94db5](https://github.com/microsoft/node-jsonc-parser/tree/3c9b4203d663061d87d4d34dd0004690aef94db5).
It is consumed as an ordinary dependency, not vendored or modified. Its visitor
checks strict JSON, duplicate keys and the supported numeric/depth profile; the
fault-tolerant parse result is never used. The pinned npm artifact integrity is
retained in the staging provenance; installation must preserve it in a lockfile.

The build-only dependency TypeScript is pinned to 5.8.3, Apache-2.0. It is not a
runtime dependency. There are no MobKit, Meerkat, home-automation host or credential-store
runtime dependencies.

The Microsoft dependency license follows for attribution.

The MIT License (MIT)

Copyright (c) Microsoft

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.

## Setup conversation Markdown

The setup conversation uses Marked 18.1.0's public Lexer and the
entities 8.1.0 strict entity decoder as ordinary pinned runtime dependencies.
Toolkit constructs DOM nodes from tokens; it does not use Marked's HTML
renderer or insert model text as HTML. Code remains verbatim, raw HTML stays
inert, and images do not cause network requests. The standalone private browser
component includes the exact parser/decoder JavaScript and these licenses.

Primary API and provenance references:

- https://marked.js.org/using_pro#the-lexer--lexer
- https://github.com/markedjs/marked/tree/v18.1.0
- https://github.com/markedjs/marked/security/advisories/GHSA-6v9c-7cg6-27q7
- https://github.com/fb55/entities/tree/v8.1.0

The npm archive integrity pins are in package-lock.json. The decoder is
BSD-2-Clause; Marked is MIT with its upstream Markdown attribution below.

# License information

## Contribution License Agreement

If you contribute code to this project, you are implicitly allowing your code
to be distributed under the MIT license. You are also implicitly verifying that
all code is your original work. `</legalese>`

## Marked

Copyright (c) 2018+, MarkedJS (https://github.com/markedjs/)
Copyright (c) 2011-2018, Christopher Jeffrey (https://github.com/chjj/)

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

## Markdown

Copyright © 2004, John Gruber
http://daringfireball.net/
All rights reserved.

Redistribution and use in source and binary forms, with or without modification, are permitted provided that the following conditions are met:

* Redistributions of source code must retain the above copyright notice, this list of conditions and the following disclaimer.
* Redistributions in binary form must reproduce the above copyright notice, this list of conditions and the following disclaimer in the documentation and/or other materials provided with the distribution.
* Neither the name “Markdown” nor the names of its contributors may be used to endorse or promote products derived from this software without specific prior written permission.

This software is provided by the copyright holders and contributors “as is” and any express or implied warranties, including, but not limited to, the implied warranties of merchantability and fitness for a particular purpose are disclaimed. In no event shall the copyright owner or contributors be liable for any direct, indirect, incidental, special, exemplary, or consequential damages (including, but not limited to, procurement of substitute goods or services; loss of use, data, or profits; or business interruption) however caused and on any theory of liability, whether in contract, strict liability, or tort (including negligence or otherwise) arising in any way out of the use of this software, even if advised of the possibility of such damage.

## Entities license

Copyright (c) Felix Böhm
All rights reserved.

Redistribution and use in source and binary forms, with or without modification, are permitted provided that the following conditions are met:

Redistributions of source code must retain the above copyright notice, this list of conditions and the following disclaimer.

Redistributions in binary form must reproduce the above copyright notice, this list of conditions and the following disclaimer in the documentation and/or other materials provided with the distribution.

THIS IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS "AS IS" AND ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT LIMITED TO, THE IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR PURPOSE ARE DISCLAIMED. IN NO EVENT SHALL THE COPYRIGHT HOLDER OR CONTRIBUTORS BE LIABLE FOR ANY DIRECT, INDIRECT, INCIDENTAL, SPECIAL, EXEMPLARY, OR CONSEQUENTIAL DAMAGES (INCLUDING, BUT NOT LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS OR SERVICES; LOSS OF USE, DATA, OR PROFITS; OR BUSINESS INTERRUPTION) HOWEVER CAUSED AND ON ANY THEORY OF LIABILITY, WHETHER IN CONTRACT, STRICT LIABILITY, OR TORT (INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT OF THE USE OF THIS,
EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGE.
