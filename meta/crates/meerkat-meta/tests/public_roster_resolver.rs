//! Reachability of the roster resolvers from OUTSIDE the crate.
//!
//! An integration test is a separate crate, so a `pub(crate)` function does not
//! compile here. That is the entire point of this file: the inline module tests
//! call these same functions happily whether they are `pub` or `pub(crate)`, so
//! they prove nothing about what an embedder can reach.
//!
//! That gap shipped. `roster_member_id_for_identity` was `pub(crate)` while
//! being the only correct way for an embedder to produce a roster key, and an
//! instruction to call it produced E0603 for a production embedder rather than a working fix - an
//! instruction that was correct and premature, which is harder to diagnose than
//! one that is simply wrong, because the compiler error has three plausible
//! causes and no way to rank them.
//!
//! If either resolver stops being public, this file fails to COMPILE. A test
//! that has to run in order to notice would already be too late.

use meerkat_meta::member_comms_id::{
    roster_member_id_for_identity, roster_member_id_for_supplied_id,
};

/// Every spelling an embedder can be holding reaches the one roster row, through
/// the PUBLIC surface.
#[test]
fn an_embedder_reaches_the_roster_row_from_any_spelling_it_holds() {
    let expected = roster_member_id_for_identity("review:singleton");

    // The durable identity: for a caller that knows which shape it holds and
    // wants to say so.
    assert_eq!(
        roster_member_id_for_identity("review:singleton"),
        expected,
        "the durable-identity resolver must be reachable and stable"
    );

    // The public runtime alias, which `status_identity` RETURNS. This is the
    // value an embedder reads back from a surface and hands to a lookup, and it
    // is the case that had no public door at 0.8.23.
    assert_eq!(
        roster_member_id_for_supplied_id("rt:review:singleton:0"),
        expected,
        "the alias our own surfaces hand out must reach the same row"
    );

    // Generation is incarnation detail and must not select a different row.
    assert_eq!(
        roster_member_id_for_supplied_id("rt:review:singleton:7"),
        expected,
        "generation must not change which roster row is named"
    );

    // The production embedder's exact persisted binding shape (only the address is a placeholder): a comms encoding
    // wrapped around a runtime alias, alias innermost. Theirs is the only store known to carry the
    // stacked shape, so an input built from this crate's own helpers cannot stand in for it.
    assert_eq!(
        roster_member_id_for_supplied_id("mk--rt_cperson_cjane_x2e_doe_x40_example_x2e_com_c2"),
        roster_member_id_for_identity("person:jane.doe@example.com"),
        "the production embedder's persisted stacked alias must reach the durable person row"
    );
}
