use axum::extract::Query;

use super::*;
use crate::imagegen::Paid;

async fn checkpoints(State(server): State<Server>, headers: HeaderMap) -> Json<Value> {
    note(&server, "/comfyui/object_info".into(), &headers, &Bytes::new());

    Json(
        json!({ "CheckpointLoaderSimple": { "input": { "required": { "ckpt_name": [["sdxl.safetensors", "other.ckpt"]] } } } }),
    )
}

async fn upload(State(server): State<Server>, headers: HeaderMap, body: Bytes) -> Json<Value> {
    note(&server, "/comfyui/upload".into(), &headers, &body);

    Json(json!({ "name": "drift-reference.png", "subfolder": "", "type": "input" }))
}

async fn queue(State(server): State<Server>, headers: HeaderMap, body: Bytes) -> Json<Value> {
    note(&server, "/comfyui/prompt".into(), &headers, &body);

    Json(json!({ "prompt_id": "p1", "number": 0 }))
}

async fn history(State(server): State<Server>, Path(id): Path<String>) -> Json<Value> {
    if server.polls.fetch_add(1, Ordering::SeqCst) == 0 {
        return Json(json!({}));
    }
    let saved = json!({ "images": [{ "filename": "drift_00001_.png", "subfolder": "", "type": "output" }] });

    Json(json!({ id: { "status": { "status_str": "success" }, "outputs": { "8": saved } } }))
}

async fn view(State(server): State<Server>, Query(query): Query<Vec<(String, String)>>) -> impl IntoResponse {
    let asked: Vec<String> = query.iter().map(|(name, value)| format!("{name}={value}")).collect();
    note(
        &server,
        format!("/comfyui/view?{}", asked.join("&")),
        &HeaderMap::new(),
        &Bytes::new(),
    );

    ([("content-type", "image/png")], tiny_png())
}

async fn webui(
    State(server): State<Server>,
    Path(action): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Json<Value> {
    note(&server, format!("/sdwebui/{action}"), &headers, &body);

    Json(json!({ "images": [png64()], "parameters": {}, "info": "{}" }))
}

pub(super) fn routes() -> axum::Router<Server> {
    axum::Router::new()
        .route("/comfyui/object_info/CheckpointLoaderSimple", get(checkpoints))
        .route("/comfyui/upload/image", post(upload))
        .route("/comfyui/prompt", post(queue))
        .route("/comfyui/history/{id}", get(history))
        .route("/comfyui/view", get(view))
        .route("/sdwebui/sdapi/v1/{action}", post(webui))
}

#[tokio::test]
async fn comfyui_runs_a_checkpoint_workflow_from_an_uploaded_reference_and_charges_nothing() {
    let (engine, server) = engine().await;
    choose(&engine, "comfyui", "");
    assert!(engine.image_generation_available(), "a local server needs no key");
    assert_eq!(
        engine.image_ask_label().as_deref(),
        Some("ComfyUI at http://127.0.0.1:8188")
    );

    let made = engine
        .generate_image(request("a red square", "16:9", vec![reference()]))
        .await
        .unwrap();
    assert_eq!((made.paid, made.cost), (Paid::Local, None));
    assert_eq!(made.image.bytes().unwrap(), tiny_png());

    let asked = asked(&server);
    let paths: Vec<&str> = asked.iter().map(|asked| asked.path.as_str()).collect();
    assert_eq!(
        paths,
        [
            "/comfyui/object_info",
            "/comfyui/upload",
            "/comfyui/prompt",
            "/comfyui/view?filename=drift_00001_.png&subfolder=&type=output"
        ]
    );
    let graph = &asked[2].body["prompt"];
    assert_eq!(
        graph["1"]["inputs"]["ckpt_name"], "sdxl.safetensors",
        "the first checkpoint it lists"
    );
    assert_eq!(graph["2"]["inputs"]["text"], "a red square");
    assert_eq!(graph["4"]["class_type"], "VAEEncode", "started from the reference");
    assert_eq!(graph["5"]["inputs"]["image"], "drift-reference.png");
    assert_eq!(graph["6"]["inputs"]["denoise"], 0.6);
}

#[tokio::test]
async fn the_webui_takes_its_checkpoint_as_an_override_and_img2img_from_a_reference() {
    let (engine, server) = engine().await;
    choose(&engine, "sdwebui", "juggernaut.safetensors");

    engine
        .generate_image(request("a red square", "9:16", Vec::new()))
        .await
        .unwrap();
    engine
        .generate_image(request("bluer", "auto", vec![reference()]))
        .await
        .unwrap();

    let asked = asked(&server);
    assert_eq!(asked[0].path, "/sdwebui/txt2img");
    assert_eq!(
        (asked[0].body["width"].clone(), asked[0].body["height"].clone()),
        (json!(576), json!(1024))
    );
    assert_eq!(
        asked[0].body["override_settings"]["sd_model_checkpoint"],
        "juggernaut.safetensors"
    );
    assert_eq!(asked[1].path, "/sdwebui/img2img");
    assert_eq!(asked[1].body["init_images"][0], reference().base64.as_str());
}

#[tokio::test]
async fn an_openai_compatible_server_needs_a_model_and_takes_a_key_when_given_one() {
    let (engine, server) = engine().await;
    choose(&engine, "compatible", "");
    let unnamed = engine.generate_image(request("a red square", "auto", Vec::new())).await;
    assert!(unnamed.unwrap_err().contains("no model is chosen"));

    choose(&engine, "compatible", "flux-schnell");
    let free = engine
        .generate_image(request("a red square", "auto", Vec::new()))
        .await
        .unwrap();
    assert_eq!((free.paid, free.cost), (Paid::Local, None));
    engine.set_image_key("compatible", "gateway-key").unwrap();
    let keyed = engine
        .generate_image(request("a red square", "auto", Vec::new()))
        .await
        .unwrap();
    assert_eq!(
        (keyed.paid, keyed.cost),
        (Paid::Billed, None),
        "its price is the server's to know"
    );

    let asked = asked(&server);
    assert_eq!(asked[0].path, "/compatible/images/generations");
    assert_eq!((asked[0].key.as_str(), asked[1].key.as_str()), ("", "gateway-key"));
    assert_eq!(asked[0].body["model"], "flux-schnell");
}
