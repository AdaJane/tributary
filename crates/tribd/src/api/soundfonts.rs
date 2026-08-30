//! Uploading and removing SoundFonts.
//!
//! The upload streams straight to a temp file beside the library and is
//! renamed into place only once it has loaded, so a dropped connection or a
//! rejected file never leaves half a soundfont behind — the same temp+rename
//! discipline the project manifest and the recording prefs use.

use axum::body::Body;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use futures::StreamExt;
use tokio::io::AsyncWriteExt;

use super::{ApiError, AppState};
use crate::api::instruments::InstrumentsDto;
use crate::soundfonts;

/// Bytes read before deciding whether this is a SoundFont at all.
///
/// The RIFF form type sits at offset 8, so twelve is enough — and checking
/// this early is the difference between refusing a 300 MB holiday video and
/// filling the disk with it first.
const HEADER_BYTES: usize = 12;

/// Take a SoundFont into the library.
///
/// The body is the file. Deliberately not multipart: the payload is one
/// opaque blob, multipart would add a dependency and a parser to say so,
/// and the name already has a home in the path.
///
/// Note that axum's `DefaultBodyLimit` does NOT apply to a body consumed as
/// a stream, so there is no limit to raise — and equally no limit at all
/// unless the handler enforces one. On a daemon with no authentication that
/// is not acceptable, so the byte budget is counted here, and exceeding it
/// aborts and unlinks rather than filling the disk.
#[utoipa::path(
    post,
    path = "/api/v1/soundfonts/{name}",
    params(("name" = String, Path, description = "File name, slugged by the daemon")),
    request_body(content = Vec<u8>, content_type = "application/octet-stream"),
    responses(
        (status = 201, description = "The library after the upload", body = InstrumentsDto),
        (status = 409, description = "Recording in progress"),
        (status = 413, description = "Larger than this installation allows"),
        (status = 422, description = "Not a SoundFont, or it would not load"),
    ),
)]
pub async fn upload_soundfont(
    State(state): State<AppState>,
    Path(name): Path<String>,
    _headers: HeaderMap,
    body: Body,
) -> Result<(StatusCode, axum::Json<InstrumentsDto>), ApiError> {
    // Loading a new soundfont rebuilds racks, which reloads samples. Not
    // something to do while tape is rolling.
    if state.control.transport().await.state == crate::api::ws::TransportPhase::Recording {
        return Err(ApiError::Conflict(
            "stop recording before loading a soundfont".into(),
        ));
    }

    let file_name = soundfonts::safe_file_name(&name).ok_or_else(|| {
        ApiError::Invalid(format!("“{name}” is not a SoundFont (.sf2) file name"))
    })?;

    // A library id names exactly one file, and this is what keeps that
    // true. Allowing an upload to shadow a shipped name would make the
    // order `list` reports and the order `resolve` searches into a rule
    // nobody could see — and an instrument's stored `soundfont` would
    // silently mean a different file than it did yesterday.
    if state.library().is_builtin(&file_name) {
        return Err(ApiError::Invalid(format!(
            "“{file_name}” is the name of a built-in sound — save yours under another name"
        )));
    }

    let dir = std::path::PathBuf::from(&state.soundfont_dir);
    // Temp file in the SAME directory: that is what makes the rename
    // atomic, and what stops a half-written file ever being listed.
    let temp = dir.join(format!(".{file_name}.part"));
    let final_path = dir.join(&file_name);

    let written = stream_to_file(body, &temp, state.max_soundfont_bytes).await;
    let outcome = match written {
        Ok(()) => validate(&temp).await,
        Err(e) => Err(e),
    };
    if let Err(e) = outcome {
        let _ = tokio::fs::remove_file(&temp).await;
        return Err(e);
    }

    tokio::fs::rename(&temp, &final_path)
        .await
        .map_err(|e| ApiError::Internal(format!("could not store the soundfont: {e}")))?;

    // A newly available soundfont may be exactly what a Missing instrument
    // has been asking for, so retry them rather than making the user press
    // Refresh to discover their own upload worked.
    state.instruments.refresh().await;
    Ok((
        StatusCode::CREATED,
        axum::Json(crate::api::instruments::document(&state).await),
    ))
}

async fn stream_to_file(
    body: Body,
    temp: &std::path::Path,
    max_bytes: u64,
) -> Result<(), ApiError> {
    let mut file = tokio::fs::File::create(temp)
        .await
        .map_err(|e| ApiError::Internal(format!("could not open the soundfont library: {e}")))?;

    let mut stream = body.into_data_stream();
    let mut written: u64 = 0;
    let mut head = Vec::with_capacity(HEADER_BYTES);

    while let Some(chunk) = stream.next().await {
        let chunk =
            chunk.map_err(|e| ApiError::Invalid(format!("the upload did not finish: {e}")))?;

        // Decide what this is from the first bytes, before committing to
        // however many hundred megabytes follow.
        if head.len() < HEADER_BYTES {
            head.extend_from_slice(&chunk[..chunk.len().min(HEADER_BYTES - head.len())]);
            if head.len() >= HEADER_BYTES && !soundfonts::has_soundfont_header(&head) {
                return Err(ApiError::Invalid(
                    "that is not a SoundFont (.sf2) file".into(),
                ));
            }
        }

        written += chunk.len() as u64;
        if written > max_bytes {
            return Err(ApiError::TooLarge(format!(
                "soundfonts on this installation are limited to {} MB",
                max_bytes / (1024 * 1024)
            )));
        }
        file.write_all(&chunk)
            .await
            .map_err(|e| ApiError::Internal(format!("could not write the soundfont: {e}")))?;
    }

    file.flush()
        .await
        .map_err(|e| ApiError::Internal(format!("could not write the soundfont: {e}")))?;

    if !soundfonts::has_soundfont_header(&head) {
        return Err(ApiError::Invalid(
            "that is not a SoundFont (.sf2) file".into(),
        ));
    }
    Ok(())
}

/// Prove it loads before it joins the library.
///
/// The real loader, not a header sniff: a file can carry a valid RIFF/sfbk
/// header and still be truncated or use a sample format this synthesiser
/// cannot play, and finding that out at upload time is far kinder than
/// finding it out as a silent instrument.
async fn validate(temp: &std::path::Path) -> Result<(), ApiError> {
    let path = temp.to_path_buf();
    tokio::task::spawn_blocking(move || {
        let mut file = std::fs::File::open(&path)?;
        trib_engine::SoundFont::new(&mut file)
            .map(|_| ())
            .map_err(std::io::Error::other)
    })
    .await
    .map_err(|e| ApiError::Internal(format!("soundfont validation did not run: {e}")))?
    .map_err(|e| ApiError::Invalid(format!("that SoundFont could not be loaded: {e}")))
}

/// Remove a soundfont from the library.
///
/// Only the internal library: a file on somebody's USB stick is theirs, and
/// the console says so rather than offering a button that deletes it.
#[utoipa::path(
    delete,
    path = "/api/v1/soundfonts/{name}",
    params(("name" = String, Path, description = "Library id")),
    responses(
        (status = 200, description = "The library after the removal", body = InstrumentsDto),
        (status = 404, description = "Not in the internal library"),
        (status = 409, description = "An instrument is using it, or tape is rolling"),
    ),
)]
pub async fn delete_soundfont(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> Result<axum::Json<InstrumentsDto>, ApiError> {
    if state.control.transport().await.state == crate::api::ws::TransportPhase::Recording {
        return Err(ApiError::Conflict(
            "stop recording before removing a soundfont".into(),
        ));
    }
    let file_name = soundfonts::safe_file_name(&name).ok_or(ApiError::NotFound)?;

    // Built-ins belong to the installation, not to the session. Removing
    // one would also make it un-restorable from the console, since there
    // is no way to upload a file back under a built-in's name.
    if state.library().is_builtin(&file_name) {
        return Err(ApiError::Conflict(
            "built-in sounds are part of this installation and cannot be removed".into(),
        ));
    }

    // Refusing while an instrument holds it keeps the failure at the moment
    // of the decision, rather than as a silent rack after the next restart.
    let snapshot = state.control.snapshot().await;
    let users: Vec<&str> = snapshot
        .instruments
        .iter()
        .filter(|i| i.soundfont.as_deref() == Some(file_name.as_str()))
        .map(|i| i.name.as_str())
        .collect();
    if !users.is_empty() {
        return Err(ApiError::Conflict(format!(
            "in use by {}",
            users.join(", ")
        )));
    }

    let path = std::path::PathBuf::from(&state.soundfont_dir).join(&file_name);
    if !path.is_file() {
        return Err(ApiError::NotFound);
    }
    tokio::fs::remove_file(&path)
        .await
        .map_err(|e| ApiError::Internal(format!("could not remove the soundfont: {e}")))?;

    state.instruments.refresh().await;
    Ok(axum::Json(crate::api::instruments::document(&state).await))
}

/// One selectable sound inside a SoundFont.
#[derive(Debug, Clone, serde::Serialize, utoipa::ToSchema)]
pub struct PresetDto {
    pub bank: u16,
    pub program: u8,
    pub name: String,
}

/// The presets inside one library file.
///
/// Its own endpoint rather than a field on `InstrumentsDto`: a General MIDI
/// bank has around 300 of these, three are bundled, and the instruments
/// document is fetched on every mixer change. The console asks for the one
/// font it is showing a picker for.
///
/// Reading them means parsing the whole file, which for a 206 MB bank is
/// real work — hence `spawn_blocking`, and hence the console asking once
/// per soundfont rather than per render.
#[utoipa::path(
    get,
    path = "/api/v1/soundfonts/{name}/presets",
    params(("name" = String, Path, description = "Library id, e.g. GeneralUser-GS.sf2")),
    responses(
        (status = 200, description = "Every preset in the file, by bank then program", body = [PresetDto]),
        (status = 404, description = "No such soundfont in the library"),
        (status = 422, description = "Present, but not a SoundFont this daemon can read"),
    )
)]
pub async fn list_presets(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> Result<axum::Json<Vec<PresetDto>>, ApiError> {
    let file_name = soundfonts::safe_file_name(&name).ok_or(ApiError::NotFound)?;
    // Looked up in the library the instrument host already enumerated,
    // rather than resolved again here. That keeps one answer to "which
    // drives count" — the host holds the mounts and this handler does not
    // — and the path is one we produced, never one a caller supplied.
    // Fonts on a stick are selectable for the same reason they are
    // playable: they are in the list.
    let path = state
        .instruments
        .library()
        .await
        .into_iter()
        .find(|sf| sf.id == file_name)
        .map(|sf| std::path::PathBuf::from(sf.path))
        .ok_or(ApiError::NotFound)?;

    tokio::task::spawn_blocking(move || {
        let mut file = std::fs::File::open(&path)
            .map_err(|e| ApiError::Internal(format!("could not read the soundfont: {e}")))?;
        let sf = trib_engine::SoundFont::new(&mut file)
            .map_err(|e| ApiError::Invalid(format!("that SoundFont could not be read: {e}")))?;
        let mut presets: Vec<PresetDto> = sf
            .get_presets()
            .iter()
            .map(|p| PresetDto {
                bank: u16::try_from(p.get_bank_number()).unwrap_or(0),
                program: u8::try_from(p.get_patch_number()).unwrap_or(0),
                name: p.get_name().to_owned(),
            })
            .collect();
        // Bank then program: General MIDI order, which is the order a
        // player expects to scroll through, not the file's storage order.
        presets.sort_by(|a, b| a.bank.cmp(&b.bank).then(a.program.cmp(&b.program)));
        Ok(axum::Json(presets))
    })
    .await
    .map_err(|e| ApiError::Internal(e.to_string()))?
}
