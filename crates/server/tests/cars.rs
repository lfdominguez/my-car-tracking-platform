//! Integration tests for car create/update validation and fuel defaults.
//! Requires DATABASE_URL pointing at Postgres+PostGIS.

mod common;

use common::{create_car, login, start_server};
use serde_json::{Value, json};

#[tokio::test]
async fn switching_to_diesel_takes_the_diesel_grade_and_its_constants() {
    let Some(base) = start_server().await else {
        eprintln!("skipping: DATABASE_URL not set or DB unavailable");
        return;
    };
    let owner = login(&base).await;
    let car_id = create_car(&base, &owner).await;

    let car: Value = owner
        .client
        .patch(format!("{base}/api/cars/{car_id}"))
        .json(&json!({ "fuel_class": "DIESEL" }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(car["fuel_class"], "DIESEL");
    assert_eq!(car["fuel_type"], "B7", "diesel must not keep E10: {car}");
    assert_eq!(car["stoich_afr"], 14.5);
    assert_eq!(car["density_gl"], 835.0);
}

#[tokio::test]
async fn battery_capacity_can_be_cleared_and_bad_numbers_are_rejected() {
    let Some(base) = start_server().await else {
        eprintln!("skipping: DATABASE_URL not set or DB unavailable");
        return;
    };
    let owner = login(&base).await;
    let car_id = create_car(&base, &owner).await;
    let patch = |body: Value| {
        owner
            .client
            .patch(format!("{base}/api/cars/{car_id}"))
            .json(&body)
            .send()
    };

    let car: Value = patch(json!({ "battery_capacity_kwh": 12.5 }))
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(car["battery_capacity_kwh"], 12.5);

    // Omitting the field keeps it ...
    let car: Value = patch(json!({ "name": "Renamed" }))
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(car["battery_capacity_kwh"], 12.5);

    // ... an explicit null clears it.
    let car: Value = patch(json!({ "battery_capacity_kwh": null }))
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(car["battery_capacity_kwh"].is_null(), "{car}");

    for bad in [
        json!({ "ve": 0.0 }),
        json!({ "displacement_l": -1.0 }),
        json!({ "stoich_afr": 0.0 }),
    ] {
        let resp = patch(bad.clone()).await.unwrap();
        assert_eq!(resp.status(), reqwest::StatusCode::BAD_REQUEST, "{bad}");
    }
}

/// Real encoder output (Pillow) with an EXIF "Make" tag: the stored photo must
/// come back without it and still be the same kind of image.
#[tokio::test]
async fn uploaded_photos_lose_their_exif() {
    let Some(base) = start_server().await else {
        eprintln!("skipping: DATABASE_URL not set or DB unavailable");
        return;
    };
    let owner = login(&base).await;
    let car_id = create_car(&base, &owner).await;

    for (name, bytes, mime) in [
        (
            "exif.jpg",
            &include_bytes!("fixtures/exif.jpg")[..],
            "image/jpeg",
        ),
        (
            "exif.png",
            &include_bytes!("fixtures/exif.png")[..],
            "image/png",
        ),
        (
            "exif.webp",
            &include_bytes!("fixtures/exif.webp")[..],
            "image/webp",
        ),
    ] {
        assert!(
            bytes.windows(9).any(|w| w == b"SECRETCAM"),
            "fixture {name}"
        );
        let form = reqwest::multipart::Form::new().part(
            "photo",
            reqwest::multipart::Part::bytes(bytes.to_vec()).file_name(name),
        );
        let resp = owner
            .client
            .post(format!("{base}/api/cars/{car_id}/photo"))
            .multipart(form)
            .send()
            .await
            .unwrap();
        assert!(resp.status().is_success(), "{name}: {}", resp.status());

        let got = owner
            .client
            .get(format!("{base}/api/cars/{car_id}/photo"))
            .send()
            .await
            .unwrap();
        assert_eq!(got.headers()["content-type"], mime);
        let stored = got.bytes().await.unwrap();
        assert!(
            !stored.windows(9).any(|w| w == b"SECRETCAM"),
            "{name} kept its EXIF"
        );
        if let Ok(dir) = std::env::var("PHOTO_DUMP_DIR") {
            std::fs::write(format!("{dir}/stripped-{name}"), &stored).unwrap();
        }
    }
}
