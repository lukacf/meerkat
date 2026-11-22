## Changed

Configured self-hosted Chat Completions and Responses clients now describe an engine-authored closed set of string choices as a string enum in the outgoing tool schema. An optional parameter still stays out of `required`. The schema no longer advertises a description for each choice or an explicit `null` alternative.

## Upgrade

If you inspect these request schemas, read the allowed strings from `enum`; do not rely on per-choice descriptions. Omission is still allowed for the optional parameter. This does not establish that sending `null` is equivalent to omitting it.

Verification covered locally constructed requests for one built-in parameter, with direct OpenAI, alternate base URL, and non-engine controls. It did not establish that a server accepts the schema or that boolean parameters change.
