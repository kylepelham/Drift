use super::*;
use crate::imagegen::Paid;

#[tokio::test]
async fn openai_generates_and_edits_with_references_as_data_urls() {
    let (engine, server) = engine().await;
    choose(&engine, "openai", "");

    let made = engine
        .generate_image(request("a red square", "16:9", Vec::new()))
        .await
        .unwrap();
    assert_eq!((made.provider, made.model.as_str()), ("OpenAI", "gpt-image-2"));
    assert_eq!((made.image.mime.as_str(), made.paid), ("image/png", Paid::Billed));
    let cost = (10.0 * 5.0 + 40.0 * 8.0 + 1000.0 * 30.0) / 1e6;
    assert!(
        (made.cost.unwrap() - cost).abs() < 1e-12,
        "priced from the answer's token counts"
    );

    let reference = reference();
    engine
        .generate_image(request("bluer", "1024x1536", vec![reference.clone()]))
        .await
        .unwrap();

    let asked = asked(&server);
    assert_eq!(asked[0].path, "/openai/images/generations");
    assert_eq!(asked[0].key, "openai-key");
    assert_eq!(
        asked[0].body,
        json!({ "model": "gpt-image-2", "prompt": "a red square", "n": 1, "output_format": "png", "size": "1536x1024" })
    );
    assert_eq!(asked[1].path, "/openai/images/edits");
    assert_eq!(asked[1].body["size"], "1024x1536");
    assert_eq!(
        asked[1].body["images"],
        json!([{ "image_url": format!("data:image/png;base64,{}", reference.base64) }])
    );
}

#[tokio::test]
async fn google_asks_generate_content_for_an_image_with_its_ratio_and_tier() {
    let (engine, server) = engine().await;
    choose(&engine, "google", "gemini-3-pro-image");
    let reference = reference();

    let made = engine
        .generate_image(request("a red square", "2000x1100", vec![reference.clone()]))
        .await
        .unwrap();
    assert_eq!(made.image.bytes().unwrap(), tiny_png());
    assert_eq!(made.revised_prompt.as_deref(), Some("Here it is."));
    let cost = (100.0 * 2.0 + 4.0 * 12.0 + 1120.0 * 120.0) / 1e6;
    assert!((made.cost.unwrap() - cost).abs() < 1e-12);

    let asked = asked(&server);
    assert_eq!(asked[0].path, "/google/models/gemini-3-pro-image:generateContent");
    assert_eq!(asked[0].key, "google-key");
    let parts = &asked[0].body["contents"][0]["parts"];
    assert_eq!(parts[0]["inlineData"]["data"], reference.base64.as_str());
    assert_eq!(parts[1]["text"], "a red square");
    assert_eq!(
        asked[0].body["generationConfig"],
        json!({ "responseModalities": ["TEXT", "IMAGE"], "imageConfig": { "aspectRatio": "16:9", "imageSize": "2K" } })
    );
}

#[tokio::test]
async fn xai_reports_its_own_cost_and_takes_one_image_or_several() {
    let (engine, server) = engine().await;
    choose(&engine, "xai", "");
    let reference = reference();

    let made = engine
        .generate_image(request("a red square", "auto", Vec::new()))
        .await
        .unwrap();
    assert_eq!(made.model, "grok-imagine-image-2.0");
    assert_eq!(made.cost, Some(0.02), "cost_in_usd_ticks, ten billion to the dollar");
    assert_eq!(made.revised_prompt.as_deref(), Some("a red square, centred"));
    engine
        .generate_image(request("edit", "auto", vec![reference.clone()]))
        .await
        .unwrap();
    engine
        .generate_image(request("combine", "4:3", vec![reference.clone(), reference]))
        .await
        .unwrap();

    let asked = asked(&server);
    assert_eq!(asked[0].path, "/xai/images/generations");
    assert_eq!(
        asked[0].body,
        json!({ "model": "grok-imagine-image-2.0", "prompt": "a red square", "n": 1, "response_format": "b64_json" })
    );
    assert_eq!(asked[1].path, "/xai/images/edits");
    assert_eq!(asked[1].body["image"]["type"], "image_url");
    assert_eq!(asked[2].body["images"].as_array().unwrap().len(), 2);
    assert_eq!(asked[2].body["aspect_ratio"], "4:3");
}

#[tokio::test]
async fn recraft_names_its_units_and_edits_from_one_reference_on_v3() {
    let (engine, server) = engine().await;
    choose(&engine, "recraft", "");

    let made = engine
        .generate_image(request("a red square", "16:9", Vec::new()))
        .await
        .unwrap();
    assert_eq!(made.cost, Some(0.035), "35 units, a thousand to the dollar");
    let edited = engine
        .generate_image(request("bluer", "auto", vec![reference()]))
        .await
        .unwrap();
    assert_eq!(edited.cost, Some(0.04));
    assert_eq!(
        edited.image.bytes().unwrap(),
        tiny_png(),
        "fetched from the URL it names"
    );
    let refused = engine
        .generate_image(request("combine", "auto", vec![reference(), reference()]))
        .await;
    assert!(refused.unwrap_err().contains("at most 1 reference"));

    let asked = asked(&server);
    assert_eq!(asked.len(), 2, "a call over the limit sends nothing");
    assert_eq!(asked[0].path, "/recraft/images/generations");
    assert_eq!(asked[0].key, "recraft-key");
    assert_eq!(
        asked[0].body,
        json!({ "prompt": "a red square", "model": "recraftv4_1", "response_format": "b64_json", "size": "1820x1024" })
    );
    assert_eq!(asked[1].path, "/recraft/images/imageToImage");
    let form = asked[1].body.as_str().unwrap();
    assert!(form.contains("name=\"model\"\r\n\r\nrecraftv3") && form.contains("name=\"image\"; filename="));
}

#[tokio::test]
async fn flux_is_polled_until_ready_and_charges_its_credits() {
    let (engine, server) = engine().await;
    choose(&engine, "bfl", "flux-2-max");

    let made = engine
        .generate_image(request("a red square", "16:9", vec![reference(), reference()]))
        .await
        .unwrap();
    assert_eq!(made.cost, Some(0.03), "three credits, a cent each");
    assert_eq!(made.image.bytes().unwrap(), tiny_png());

    let asked = asked(&server);
    assert_eq!(asked[0].path, "/bfl/flux-2-max");
    assert_eq!(asked[0].key, "bfl-key");
    assert_eq!(
        (asked[0].body["width"].clone(), asked[0].body["height"].clone()),
        (json!(1024), json!(576))
    );
    assert_eq!(asked[0].body["input_image"], reference().base64.as_str());
    assert!(asked[0].body["input_image_2"].is_string() && asked[0].body["input_image_3"].is_null());
    assert_eq!(
        asked.iter().filter(|asked| asked.path == "/bfl/poll").count(),
        2,
        "pending, then ready"
    );
}

#[tokio::test]
async fn stability_sends_a_form_per_model_and_core_takes_no_reference() {
    let (engine, server) = engine().await;
    choose(&engine, "stability", "sd3.5-large");

    let made = engine
        .generate_image(request("a red square", "16:9", Vec::new()))
        .await
        .unwrap();
    assert_eq!(made.cost, Some(0.065), "6.5 credits");
    engine
        .generate_image(request("bluer", "auto", vec![reference()]))
        .await
        .unwrap();
    choose(&engine, "stability", "core");
    let refused = engine.generate_image(request("bluer", "auto", vec![reference()])).await;
    assert!(refused.unwrap_err().contains("text only"));

    let asked = asked(&server);
    assert_eq!(asked.len(), 2);
    assert_eq!(asked[0].path, "/stability/sd3");
    assert_eq!(asked[0].headers["accept"], "application/json");
    let form = asked[0].body.as_str().unwrap();
    assert!(form.contains("name=\"model\"\r\n\r\nsd3.5-large") && form.contains("name=\"aspect_ratio\"\r\n\r\n16:9"));
    let edit = asked[1].body.as_str().unwrap();
    assert!(edit.contains("name=\"mode\"\r\n\r\nimage-to-image") && edit.contains("name=\"strength\""));
}

#[tokio::test]
async fn ideogram_and_fal_fetch_the_image_they_name_and_charge_nothing_they_do_not_say() {
    let (engine, server) = engine().await;
    choose(&engine, "ideogram", "QUALITY");
    let made = engine
        .generate_image(request("a red square", "16:9", vec![reference()]))
        .await
        .unwrap();
    assert_eq!((made.cost, made.paid), (None, Paid::Billed));
    assert_eq!(made.revised_prompt.as_deref(), Some("a red square, flat"));

    choose(&engine, "fal", "fal-ai/flux/dev");
    let made = engine
        .generate_image(request("a red square", "1024x768", Vec::new()))
        .await
        .unwrap();
    assert_eq!(made.image.bytes().unwrap(), tiny_png(), "a data URL in the answer");

    let asked = asked(&server);
    assert_eq!(asked[0].path, "/ideogram/remix");
    assert_eq!(asked[0].headers["api-key"], "ideogram-key");
    let form = asked[0].body.as_str().unwrap();
    assert!(
        form.contains("name=\"rendering_speed\"\r\n\r\nQUALITY") && form.contains("name=\"aspect_ratio\"\r\n\r\n16x9")
    );
    assert_eq!(asked[1].path, "/fal/fal-ai/flux/dev");
    assert_eq!(asked[1].headers["authorization"], "Key fal-key");
    assert_eq!(asked[1].body["image_size"], json!({ "width": 1024, "height": 768 }));
}
