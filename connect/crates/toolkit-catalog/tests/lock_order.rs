//! The one rank table and the lock-order model agree (I10 drift row).

use toolkit_catalog::lock_order::{SOURCE_OWNER_LOCK_ORDER, lock_rank};

/// The `Rank == [...]` record of the model, as (lock, rank) pairs in its order.
fn model_ranks() -> Vec<(String, u8)> {
    let model = include_str!("../models/lock_order.tla");
    let start = model.find("Rank == [").expect("the model declares Rank");
    let body = &model[start + "Rank == [".len()..];
    let end = body.find(']').expect("the Rank record closes");
    body[..end]
        .lines()
        .map(|line| line.split("\\*").next().unwrap_or_default())
        .flat_map(|line| line.split(','))
        .filter_map(|field| {
            let (lock, rank) = field.split_once("|->")?;
            Some((
                lock.trim().to_owned(),
                rank.trim().parse().expect("a rank is a number"),
            ))
        })
        .collect()
}

#[test]
fn the_lock_order_model_states_exactly_the_rank_table() {
    let table: Vec<(String, u8)> = SOURCE_OWNER_LOCK_ORDER
        .iter()
        .map(|entry| (entry.lock.to_owned(), entry.rank))
        .collect();
    assert_eq!(model_ranks(), table);
}

#[test]
fn the_rank_table_is_a_strict_total_order() {
    let mut ranks: Vec<u8> = SOURCE_OWNER_LOCK_ORDER
        .iter()
        .map(|entry| entry.rank)
        .collect();
    ranks.dedup();
    assert_eq!(
        ranks.len(),
        SOURCE_OWNER_LOCK_ORDER.len(),
        "ranks are distinct"
    );
    assert!(
        ranks.windows(2).all(|pair| pair[0] < pair[1]),
        "listed lowest first"
    );
    assert!(
        SOURCE_OWNER_LOCK_ORDER
            .iter()
            .all(|entry| lock_rank(entry.lock) == Some(entry.rank))
    );
    assert_eq!(lock_rank("PUB").zip(lock_rank("CONN")), Some((3, 4)));
    assert_eq!(lock_rank("unknown"), None);
}

#[test]
fn the_source_writer_precedes_the_class_callback_exclusion() {
    // The code's documented order: the class input path and the native and peer
    // operation writers hold the source writer while the class callback exclusion
    // is taken ("Source writer precedes this existing policy exclusion").
    assert_eq!(lock_rank("SW").zip(lock_rank("CX")), Some((1, 2)));
}
