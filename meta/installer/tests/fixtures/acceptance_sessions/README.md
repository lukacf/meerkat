These frozen generated-source fixtures reproduce the first application's offline
resource-ownership and revocation defects and its later repair. They are test
data, not installer design templates or live device adapters.

Original build: 0dcb5922-d8b4-48e7-8a86-a15601df12d1.
Repaired build: ed313830-2a20-4e7c-9ebd-9dbb15045cc0.
Only the generated ledger and integration modules are retained. The independent
acceptance tests are written separately and execute through the host sandbox.
No device, network, provider, authentication or physical behavior is proven.
