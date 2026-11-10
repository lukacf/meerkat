//! The resolver of the provider-neutral `reasoning_effort` option: which
//! source states the levels a route accepts, which typed field each provider
//! API takes a level in, and the one function that decides, for a requested
//! level, a model and a route observation, what to write on a provider
//! request or the typed refusal.

#[cfg(test)]
mod tests;
