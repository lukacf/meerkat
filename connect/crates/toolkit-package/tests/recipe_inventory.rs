use serde_json::{Value, json};
use toolkit_package::{
    CatalogOrigin, ContentDigest, Id, IntegrationBundle, IntegrationLibrary, PackageCatalog,
    RecipeInventory, RecipeInventoryError, Revision, bundled_adopted_catalog,
    bundled_discovery_catalog, bundled_integration_library, bundled_recipe_catalog, canonical_json,
};

fn list(inventory: &RecipeInventory, value: Value) -> Result<Value, ()> {
    let request = serde_json::from_value(value).map_err(|_| ())?;
    serde_json::to_value(inventory.list(request).map_err(|_| ())?).map_err(|_| ())
}

fn read(inventory: &RecipeInventory, value: Value) -> Result<Value, ()> {
    let request = serde_json::from_value(value).map_err(|_| ())?;
    serde_json::to_value(inventory.read(request).map_err(|_| ())?).map_err(|_| ())
}

#[test]
fn both_canonical_catalogs_are_read_whole_in_identity_order_without_relabeling() {
    let adopted = bundled_adopted_catalog().unwrap();
    let discovery = bundled_discovery_catalog().unwrap();
    let catalog = bundled_recipe_catalog().unwrap();
    assert_eq!(adopted.entries().count(), 101);
    assert_eq!(discovery.entries().count(), 10);
    assert_eq!(catalog.entries().count(), 111);
    for entry in adopted.entries().chain(discovery.entries()) {
        assert_eq!(catalog.get(&entry.package.identity), Some(entry));
    }
    for entry in discovery.entries() {
        assert!(adopted.get(&entry.package.identity).is_none());
    }
    let inventory = RecipeInventory::new(&catalog).unwrap();
    let entries: Vec<_> = catalog.entries().collect();
    let snapshot = ContentDigest::of_bytes(&canonical_json(&entries).unwrap());
    let mut request = json!({"limit":20});
    let mut observed = Vec::new();
    let mut listed_bytes = 0;
    loop {
        let page = list(&inventory, request).unwrap();
        assert_eq!(page["snapshot"], json!(snapshot));
        let page_bytes = serde_json::to_vec(&page).unwrap().len();
        assert!(page_bytes <= 12 * 1024);
        listed_bytes += page_bytes;
        for summary in page["entries"].as_array().unwrap() {
            let full = read(
                &inventory,
                json!({"identity":summary["identity"],"digest":summary["digest"]}),
            )
            .unwrap();
            let identity = serde_json::from_value(summary["identity"].clone()).unwrap();
            let entry = catalog.get(&identity).unwrap();
            assert_eq!(
                canonical_json(&full).unwrap(),
                canonical_json(entry).unwrap()
            );
            assert!(serde_json::to_vec(&full).unwrap().len() <= 32 * 1024);
            assert_eq!(
                summary,
                &json!({
                    "identity":entry.package.identity,"digest":entry.digest,
                    "display_name":entry.package.display_name,"description":entry.package.description,
                    "roles":entry.package.known_exports.iter().map(|export| export.role).collect::<Vec<_>>(),
                })
            );
            assert!(full.get("library").is_none());
            observed.push(identity);
        }
        if page["next_after"].is_null() {
            break;
        }
        request = json!({"limit":20,"snapshot":page["snapshot"],"after":page["next_after"]});
    }
    assert_eq!(
        observed,
        entries
            .iter()
            .map(|e| e.package.identity.clone())
            .collect::<Vec<_>>()
    );
    // The failed native trial listed 236,080 bytes before reading Wikipedia.
    // Bound the real whole catalog, not just a small synthetic summary.
    assert!(
        listed_bytes <= 64 * 1024,
        "catalog listed {listed_bytes} bytes"
    );
    let wikipedia = list(&inventory, json!({"limit":20,"name":"wikipedia"})).unwrap();
    assert_eq!(wikipedia["entries"].as_array().unwrap().len(), 1);
    assert_eq!(wikipedia["entries"][0]["identity"]["name"], "wikipedia");
    assert!(wikipedia["next_after"].is_null());
    assert!(serde_json::to_vec(&wikipedia).unwrap().len() <= 1024);
}

#[test]
fn exact_name_preserves_all_publishers_versions_and_binds_pagination_filter() {
    let supplied = bundled_recipe_catalog().unwrap();
    let original = supplied
        .entries()
        .find(|e| e.package.identity.name.as_str() == "wikipedia")
        .unwrap();
    let mut catalog = PackageCatalog::default();
    for (publisher, name, version) in [
        ("a", "unrelated", "1"),
        ("a", "wikipedia", "1"),
        ("a", "wikipedia", "2"),
        ("b", "other", "1"),
        ("b", "wikipedia", "1"),
        ("c", "wikipedia", "3"),
    ] {
        let mut package = original.package.clone();
        package.identity.publisher = Id::new(publisher).unwrap();
        package.identity.name = Id::new(name).unwrap();
        package.identity.version = Revision::new(version).unwrap();
        catalog.insert(package, original.origin.clone()).unwrap();
    }
    let inventory = RecipeInventory::new(&catalog).unwrap();
    let unfiltered = list(&inventory, json!({"limit":1})).unwrap();
    let first = list(&inventory, json!({"limit":2,"name":"wikipedia"})).unwrap();
    assert_ne!(first["snapshot"], unfiltered["snapshot"]);
    let second = list(
        &inventory,
        json!({
            "limit":2,"name":"wikipedia","snapshot":first["snapshot"],"after":first["next_after"]
        }),
    )
    .unwrap();
    assert_eq!(second["snapshot"], first["snapshot"]);
    assert!(second["next_after"].is_null());
    let actual: Vec<_> = first["entries"]
        .as_array()
        .unwrap()
        .iter()
        .chain(second["entries"].as_array().unwrap())
        .map(|summary| {
            let full = read(
                &inventory,
                json!({"identity":summary["identity"],"digest":summary["digest"]}),
            )
            .unwrap();
            let identity = serde_json::from_value(summary["identity"].clone()).unwrap();
            assert_eq!(
                full,
                serde_json::to_value(catalog.get(&identity).unwrap()).unwrap()
            );
            identity
        })
        .collect();
    let expected: Vec<_> = catalog
        .entries()
        .filter(|entry| entry.package.identity.name.as_str() == "wikipedia")
        .map(|entry| entry.package.identity.clone())
        .collect();
    assert_eq!(actual, expected);
    assert_eq!(actual.len(), 4);

    for request in [
        json!({"limit":2,"snapshot":first["snapshot"],"after":first["next_after"]}),
        json!({"limit":2,"name":"other","snapshot":first["snapshot"],"after":first["next_after"]}),
        json!({"limit":2,"name":"wikipedia","snapshot":unfiltered["snapshot"],"after":first["next_after"]}),
        json!({"limit":2,"name":"wikipedia","snapshot":first["snapshot"],"after":unfiltered["next_after"]}),
        json!({"limit":2,"name":"wikipedia","after":first["next_after"]}),
    ] {
        assert!(list(&inventory, request).is_err());
    }
    for name in ["wiki", "Wikipedia", "does-not-exist"] {
        let absent = list(&inventory, json!({"limit":20,"name":name})).unwrap();
        assert_eq!(absent["entries"], json!([]));
        assert!(absent["next_after"].is_null());
    }
    for name in [json!(""), json!("check wikipedia"), json!(1)] {
        assert!(list(&inventory, json!({"limit":20,"name":name})).is_err());
    }

    let mut changed = PackageCatalog::default();
    for entry in catalog.entries() {
        changed
            .insert(
                entry.package.clone(),
                CatalogOrigin::Curated {
                    catalog: Id::new("changed-origin").unwrap(),
                    revision: Revision::new("2").unwrap(),
                },
            )
            .unwrap();
    }
    let changed = RecipeInventory::new(&changed).unwrap();
    assert!(list(&changed, json!({
        "limit":2,"name":"wikipedia","snapshot":first["snapshot"],"after":first["next_after"]
    })).is_err());
}

#[test]
fn stale_and_malformed_selectors_cannot_choose_other_content_or_provenance() {
    let catalog = bundled_recipe_catalog().unwrap();
    let inventory = RecipeInventory::new(&catalog).unwrap();
    let page = list(&inventory, json!({"limit":1})).unwrap();
    let first = &page["entries"][0];
    let mut unknown = first["identity"].clone();
    unknown["name"] = json!("missing");
    for request in [
        json!({"limit":0}),
        json!({"limit":21}),
        json!([]),
        json!({"limit":1,"request":"choose email"}),
        json!({"limit":1,"authority":"admin"}),
        json!({"limit":1,"snapshot":"bad"}),
        json!({"limit":1,"snapshot":"0".repeat(64)}),
        json!({"limit":1,"after":first["identity"]}),
        json!({"limit":1,"snapshot":page["snapshot"],"after":unknown}),
    ] {
        assert!(list(&inventory, request).is_err());
    }
    for request in [
        json!({"identity":first["identity"],"digest":"0".repeat(64)}),
        json!({"identity":unknown,"digest":first["digest"]}),
        json!({"identity":first["identity"],"digest":first["digest"],"library_digest":"0".repeat(64)}),
        json!({"identity":first["identity"],"digest":first["digest"],"authority":"admin"}),
    ] {
        assert!(read(&inventory, request).is_err());
    }
    let mut changed = PackageCatalog::default();
    for entry in catalog.entries() {
        changed
            .insert(
                entry.package.clone(),
                CatalogOrigin::Curated {
                    catalog: Id::new("test/replacement").unwrap(),
                    revision: Revision::new("2").unwrap(),
                },
            )
            .unwrap();
    }
    let replacement = RecipeInventory::new(&changed).unwrap();
    let cursor = json!({"limit":1,"snapshot":page["snapshot"],"after":first["identity"]});
    assert!(list(&replacement, cursor.clone()).is_err());
    assert!(list(&inventory, cursor).is_ok());
}

#[test]
fn retained_bundles_require_both_digests_and_changed_prerequisites_change_snapshot() {
    let library = bundled_integration_library().unwrap();
    let inventory = RecipeInventory::from_library(&library).unwrap();
    let mut cursor = json!({"limit":20});
    let mut count = 0;
    loop {
        let page = list(&inventory, cursor).unwrap();
        for summary in page["entries"].as_array().unwrap() {
            let request = json!({"identity":summary["identity"],"digest":summary["digest"],"library_digest":summary["library_digest"]});
            let full = read(&inventory, request.clone()).unwrap();
            let identity = serde_json::from_value(summary["identity"].clone()).unwrap();
            let digest = serde_json::from_value(summary["library_digest"].clone()).unwrap();
            let expected = library.get(&identity, &digest).unwrap();
            assert_eq!(full["library"], serde_json::to_value(expected).unwrap());
            let mut entry = full.clone();
            entry.as_object_mut().unwrap().remove("library");
            assert_eq!(
                entry,
                serde_json::to_value(library.catalog().get(&identity).unwrap()).unwrap()
            );
            for field in ["digest", "library_digest"] {
                let mut stale = request.clone();
                stale[field] = json!("0".repeat(64));
                assert!(read(&inventory, stale).is_err());
            }
            let mut omitted = request;
            omitted.as_object_mut().unwrap().remove("library_digest");
            assert!(read(&inventory, omitted).is_err());
            count += 1;
        }
        if page["next_after"].is_null() {
            break;
        }
        cursor = json!({"limit":20,"snapshot":page["snapshot"],"after":page["next_after"]});
    }
    assert_eq!(count, library.entries().count());

    const PLUGIN: &[u8] =
        include_bytes!("../integrations/connectors/github-readonly/0.1.0/plugin.json");
    const MCP: &[u8] = include_bytes!("../integrations/connectors/github-readonly/0.1.0/mcp.json");
    const RECIPES: &[u8] =
        include_bytes!("../integrations/connectors/github-readonly/0.1.0/recipes.json");
    let origin = CatalogOrigin::Curated {
        catalog: Id::new("test/library").unwrap(),
        revision: Revision::new("1").unwrap(),
    };
    let mut original = IntegrationLibrary::new(origin.clone());
    original
        .insert(IntegrationBundle::parse(PLUGIN, Some(MCP), RECIPES).unwrap())
        .unwrap();
    let original = RecipeInventory::from_library(&original).unwrap();
    let page = list(&original, json!({"limit":1})).unwrap();
    let name = page["entries"][0]["identity"]["name"].clone();
    let named = list(&original, json!({"limit":1,"name":name})).unwrap();
    assert_eq!(named["entries"], page["entries"]);
    let mut recipes: Value = serde_json::from_slice(RECIPES).unwrap();
    recipes["recipes"][0]["prerequisites"][0]["instruction"] = json!("Changed host requirement");
    let mut changed = IntegrationLibrary::new(origin);
    changed
        .insert(
            IntegrationBundle::parse(PLUGIN, Some(MCP), &serde_json::to_vec(&recipes).unwrap())
                .unwrap(),
        )
        .unwrap();
    let changed = RecipeInventory::from_library(&changed).unwrap();
    assert!(list(&changed, json!({"limit":1,"snapshot":page["snapshot"]})).is_err());
    assert!(list(&original, json!({"limit":1,"snapshot":page["snapshot"]})).is_ok());
    assert!(
        list(
            &changed,
            json!({"limit":1,"name":name,"snapshot":named["snapshot"]})
        )
        .is_err()
    );
    assert!(
        list(
            &original,
            json!({"limit":1,"name":name,"snapshot":named["snapshot"]})
        )
        .is_ok()
    );
}

#[test]
fn empty_and_oversized_inventory_preserve_existing_bounds() {
    let empty = RecipeInventory::new(&PackageCatalog::default()).unwrap();
    let page = list(&empty, json!({"limit":20})).unwrap();
    assert_eq!(page["entries"], json!([]));
    assert!(page["next_after"].is_null());
    let catalog = bundled_discovery_catalog().unwrap();
    let original = catalog.entries().next().unwrap();
    let mut package = original.package.clone();
    package.strategies[0].instructions = "x".repeat(1024 * 1024);
    let mut oversized = PackageCatalog::default();
    oversized.insert(package, original.origin.clone()).unwrap();
    assert!(matches!(
        RecipeInventory::new(&oversized),
        Err(RecipeInventoryError::MetadataTooLarge)
    ));
}
