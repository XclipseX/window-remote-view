//! JNI entry points for `com.remoteview.client.NativeBridge`.
//!
//! Callbacks into Kotlin go through the cached `JavaVM` + a `GlobalRef` to the
//! `NativeBridge` class + cached static method IDs (resolved in `JNI_OnLoad`,
//! because `FindClass` from a Rust-created thread would use the wrong class
//! loader). The session worker thread attaches itself permanently.

use std::collections::HashMap;
use std::ffi::c_void;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use jni::objects::{GlobalRef, JByteArray, JClass, JObject, JStaticMethodID, JString, JValue};
use jni::signature::{Primitive, ReturnType};
use jni::sys::{jboolean, jclass, jfloat, jint, jlong, JNI_VERSION_1_6};
use jni::{JNIEnv, JavaVM};
use rv_proto::ControlMessage;
use rv_session::DatagramIo;

use crate::glue::{state, EventSink, HelloConfig, UiInput};
use crate::worker::{self, Command, WorkerHandle};

const BRIDGE_CLASS: &str = "com/remoteview/client/NativeBridge";

struct JavaCache {
    vm: JavaVM,
    class: GlobalRef,
    on_video: JStaticMethodID,
    on_state: JStaticMethodID,
    on_send: JStaticMethodID,
    on_audio: JStaticMethodID,
    on_stats: JStaticMethodID,
    /// Optional: older `NativeBridge` builds lack the cursor callbacks.
    on_cursor_shape: Option<JStaticMethodID>,
    on_cursor_pos: Option<JStaticMethodID>,
}

static CACHE: OnceLock<JavaCache> = OnceLock::new();

#[no_mangle]
pub extern "system" fn JNI_OnLoad(vm: JavaVM, _reserved: *mut c_void) -> jint {
    #[cfg(target_os = "android")]
    android_logger::init_once(
        android_logger::Config::default()
            .with_max_level(log::LevelFilter::Info)
            .with_tag("RvCore"),
    );
    match init_cache(vm) {
        Ok(c) => {
            let _ = CACHE.set(c);
            JNI_VERSION_1_6
        }
        Err(e) => {
            log::error!("JNI_OnLoad failed: {e}");
            jni::sys::JNI_ERR
        }
    }
}

fn init_cache(vm: JavaVM) -> jni::errors::Result<JavaCache> {
    let mut env = vm.get_env()?;
    let class = env.find_class(BRIDGE_CLASS)?;
    let global = env.new_global_ref(&class)?;
    let on_video = env.get_static_method_id(&class, "onVideoFrameFromNative", "([BJI)V")?;
    let on_state = env.get_static_method_id(&class, "onConnectionStateFromNative", "(ILjava/lang/String;)V")?;
    let on_send = env.get_static_method_id(&class, "onSendDatagramFromNative", "([B)V")?;
    let on_audio = env.get_static_method_id(&class, "onAudioFromNative", "([BJI)V")?;
    let on_stats = env.get_static_method_id(&class, "onStatsFromNative", "(FIF)V")?;
    let on_cursor_shape = optional_method(&mut env, &class, "onCursorShapeFromNative", "(IIII[B)V");
    let on_cursor_pos = optional_method(&mut env, &class, "onCursorPosFromNative", "(IIZ)V");
    Ok(JavaCache { vm, class: global, on_video, on_state, on_send, on_audio, on_stats, on_cursor_shape, on_cursor_pos })
}

/// Resolves a callback that may be missing at runtime: logs and returns `None`
/// (clearing the pending `NoSuchMethodError`) instead of failing `JNI_OnLoad`.
fn optional_method(env: &mut JNIEnv, class: &JClass, name: &str, sig: &str) -> Option<JStaticMethodID> {
    match env.get_static_method_id(class, name, sig) {
        Ok(id) => Some(id),
        Err(e) => {
            if env.exception_check().unwrap_or(false) {
                let _ = env.exception_clear();
            }
            log::warn!("NativeBridge.{name}{sig} not found ({e}); this callback is disabled");
            None
        }
    }
}

impl JavaCache {
    /// Attach (idempotent, permanent) and run `f` inside a local reference
    /// frame so a long-lived worker thread never leaks local refs.
    fn call<F>(&self, name: &'static str, f: F)
    where
        F: FnOnce(&mut JNIEnv, &JClass) -> jni::errors::Result<()>,
    {
        let Ok(mut env) = self.vm.attach_current_thread_permanently() else {
            return;
        };
        let cls = unsafe { JClass::from_raw(self.class.as_obj().as_raw() as jclass) };
        let r = env.with_local_frame(8, |env| f(env, &cls));
        if env.exception_check().unwrap_or(false) {
            log::warn!("Java exception in JNI callback `{name}` (cleared; see logcat)");
            let _ = env.exception_describe();
            let _ = env.exception_clear();
        }
        if let Err(e) = r {
            log::warn!("callback failed: {e}");
        }
    }

    fn void(
        env: &mut JNIEnv,
        cls: &JClass,
        id: JStaticMethodID,
        args: &[jni::sys::jvalue],
    ) -> jni::errors::Result<()> {
        unsafe { env.call_static_method_unchecked(cls, id, ReturnType::Primitive(Primitive::Void), args) }?;
        Ok(())
    }
}

/// `DatagramIo` that hands outgoing datagrams to `NativeBridge.onSendDatagramFromNative`.
pub struct JniIo(&'static JavaCache);

impl DatagramIo for JniIo {
    fn send(&mut self, buf: &[u8]) {
        let c = self.0;
        c.call("send", |env, cls| {
            let arr = env.byte_array_from_slice(buf)?;
            JavaCache::void(env, cls, c.on_send, &[JValue::Object(&arr).as_jni()])
        });
    }
}

/// `EventSink` that calls the other `*FromNative` callbacks.
pub struct JniSink(&'static JavaCache);

impl EventSink for JniSink {
    fn video(&mut self, data: &[u8], pts_us: i64, flags: i32) {
        let c = self.0;
        c.call("video", |env, cls| {
            let arr = env.byte_array_from_slice(data)?;
            JavaCache::void(
                env,
                cls,
                c.on_video,
                &[JValue::Object(&arr).as_jni(), JValue::Long(pts_us).as_jni(), JValue::Int(flags).as_jni()],
            )
        });
    }

    fn audio(&mut self, data: &[u8], pts_us: i64, codec: i32) {
        let c = self.0;
        c.call("audio", |env, cls| {
            let arr = env.byte_array_from_slice(data)?;
            JavaCache::void(
                env,
                cls,
                c.on_audio,
                &[JValue::Object(&arr).as_jni(), JValue::Long(pts_us).as_jni(), JValue::Int(codec).as_jni()],
            )
        });
    }

    fn state(&mut self, st: i32, message: Option<&str>) {
        let c = self.0;
        c.call("state", |env, cls| {
            let msg: JObject = match message {
                Some(m) => env.new_string(m)?.into(),
                None => JObject::null(),
            };
            JavaCache::void(env, cls, c.on_state, &[JValue::Int(st).as_jni(), JValue::Object(&msg).as_jni()])
        });
    }

    fn cursor_shape(&mut self, w: i32, h: i32, hot_x: i32, hot_y: i32, rgba: &[u8]) {
        let c = self.0;
        let Some(id) = c.on_cursor_shape else { return };
        c.call("cursor_shape", |env, cls| {
            let arr = env.byte_array_from_slice(rgba)?;
            JavaCache::void(
                env,
                cls,
                id,
                &[
                    JValue::Int(w).as_jni(),
                    JValue::Int(h).as_jni(),
                    JValue::Int(hot_x).as_jni(),
                    JValue::Int(hot_y).as_jni(),
                    JValue::Object(&arr).as_jni(),
                ],
            )
        });
    }

    fn cursor_pos(&mut self, x: i32, y: i32, visible: bool) {
        let c = self.0;
        let Some(id) = c.on_cursor_pos else { return };
        c.call("cursor_pos", |env, cls| {
            JavaCache::void(
                env,
                cls,
                id,
                &[JValue::Int(x).as_jni(), JValue::Int(y).as_jni(), JValue::Bool(visible as jboolean).as_jni()],
            )
        });
    }

    fn stats(&mut self, rtt_ms: f32, bitrate_kbps: i32, loss_percent: f32) {
        let c = self.0;
        c.call("stats", |env, cls| {
            JavaCache::void(
                env,
                cls,
                c.on_stats,
                &[
                    JValue::Float(rtt_ms).as_jni(),
                    JValue::Int(bitrate_kbps).as_jni(),
                    JValue::Float(loss_percent).as_jni(),
                ],
            )
        });
    }
}

// --- session registry: Kotlin holds an opaque id, never a raw pointer ---

static SESSIONS: Mutex<Option<HashMap<i64, Arc<WorkerHandle>>>> = Mutex::new(None);
static NEXT_ID: AtomicI64 = AtomicI64::new(1);

fn lookup(handle: jlong) -> Option<Arc<WorkerHandle>> {
    SESSIONS.lock().ok()?.as_ref()?.get(&handle).cloned()
}

fn send_cmd(handle: jlong, c: Command) {
    if let Some(w) = lookup(handle) {
        w.send(c);
    }
}

fn send_input(handle: jlong, i: UiInput) {
    send_cmd(handle, Command::Input(i));
}

// --- Session ---

/// LAN/server sessions are not implemented in the core yet (USB only).
#[no_mangle]
pub extern "system" fn Java_com_remoteview_client_NativeBridge_nativeConnect<'l>(
    _env: JNIEnv<'l>,
    _this: JObject<'l>,
    _target: JString<'l>,
    _config_json: JString<'l>,
) -> jlong {
    log::warn!("nativeConnect: only the USB link (nativeConnectLink) is implemented");
    if let Some(c) = CACHE.get() {
        JniSink(c).state(state::FAILED, Some("LAN/server transport not implemented; use USB"));
    }
    0
}

#[no_mangle]
pub extern "system" fn Java_com_remoteview_client_NativeBridge_nativeConnectLink<'l>(
    mut env: JNIEnv<'l>,
    _this: JObject<'l>,
    hello_json: JString<'l>,
) -> jlong {
    let Some(cache) = CACHE.get() else {
        return 0;
    };
    let json: String = match env.get_string(&hello_json) {
        Ok(s) => s.into(),
        Err(_) => return 0,
    };
    let result = HelloConfig::parse(&json).and_then(|cfg| worker::spawn(JniIo(cache), JniSink(cache), cfg));
    match result {
        Ok(w) => {
            let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
            if let Ok(mut g) = SESSIONS.lock() {
                g.get_or_insert_with(HashMap::new).insert(id, Arc::new(w));
                return id;
            }
            0
        }
        Err(e) => {
            log::warn!("nativeConnectLink: {e}");
            JniSink(cache).state(state::FAILED, Some(&e));
            0
        }
    }
}

#[no_mangle]
pub extern "system" fn Java_com_remoteview_client_NativeBridge_nativeDisconnect<'l>(
    _env: JNIEnv<'l>,
    _this: JObject<'l>,
    handle: jlong,
) {
    let w = SESSIONS.lock().ok().and_then(|mut g| g.as_mut()?.remove(&handle));
    if let Some(w) = w {
        // Last Arc owner joins the worker via Drop; in-flight users finish first.
        w.send(Command::Shutdown);
        drop(w);
    }
}

#[no_mangle]
pub extern "system" fn Java_com_remoteview_client_NativeBridge_nativePauseVideo<'l>(
    _env: JNIEnv<'l>,
    _this: JObject<'l>,
    handle: jlong,
    paused: jboolean,
) {
    send_cmd(handle, Command::Control(ControlMessage::PauseVideo(paused != 0)));
}

#[no_mangle]
pub extern "system" fn Java_com_remoteview_client_NativeBridge_nativeRequestKeyFrame<'l>(
    _env: JNIEnv<'l>,
    _this: JObject<'l>,
    handle: jlong,
) {
    send_cmd(handle, Command::Control(ControlMessage::RequestIdr));
}

/// Uploads a log file to the host (`LogChunk` control messages, saved there as
/// `android-<name>.log`). Returns false when the session handle is unknown.
#[no_mangle]
pub extern "system" fn Java_com_remoteview_client_NativeBridge_nativeUploadLog<'l>(
    mut env: JNIEnv<'l>,
    _this: JObject<'l>,
    handle: jlong,
    name: JString<'l>,
    data: JByteArray<'l>,
) -> jboolean {
    let Some(w) = lookup(handle) else { return 0 };
    let name: String = match env.get_string(&name) {
        Ok(s) => s.into(),
        Err(_) => return 0,
    };
    let Ok(bytes) = env.convert_byte_array(&data) else { return 0 };
    jboolean::from(w.send(Command::UploadLog { name, data: bytes }))
}

/// Host currently ignores quality updates (fixed bitrate); not forwarded.
#[no_mangle]
pub extern "system" fn Java_com_remoteview_client_NativeBridge_nativeSetBitrate<'l>(
    _env: JNIEnv<'l>,
    _this: JObject<'l>,
    _handle: jlong,
    _kbps: jint,
) {
}

#[no_mangle]
pub extern "system" fn Java_com_remoteview_client_NativeBridge_nativeSetMaxFps<'l>(
    _env: JNIEnv<'l>,
    _this: JObject<'l>,
    _handle: jlong,
    _fps: jint,
) {
}

// --- Datagram link ---

#[no_mangle]
pub extern "system" fn Java_com_remoteview_client_NativeBridge_nativePushDatagram<'l>(
    env: JNIEnv<'l>,
    _this: JObject<'l>,
    handle: jlong,
    data: JByteArray<'l>,
) {
    if let Ok(v) = env.convert_byte_array(&data) {
        send_cmd(handle, Command::Datagram(v));
    }
}

// --- Discovery / network (not needed for USB) ---

#[no_mangle]
pub extern "system" fn Java_com_remoteview_client_NativeBridge_nativeStartDiscovery<'l>(
    _env: JNIEnv<'l>,
    _this: JObject<'l>,
) {
}

#[no_mangle]
pub extern "system" fn Java_com_remoteview_client_NativeBridge_nativeStopDiscovery<'l>(
    _env: JNIEnv<'l>,
    _this: JObject<'l>,
) {
}

#[no_mangle]
pub extern "system" fn Java_com_remoteview_client_NativeBridge_nativeSetCellularNetwork<'l>(
    _env: JNIEnv<'l>,
    _this: JObject<'l>,
    _network_handle: jlong,
) {
}

// --- Input ---

#[no_mangle]
pub extern "system" fn Java_com_remoteview_client_NativeBridge_nativeSendKey<'l>(
    _env: JNIEnv<'l>,
    _this: JObject<'l>,
    handle: jlong,
    scan_code: jint,
    extended: jboolean,
    down: jboolean,
) {
    send_input(handle, UiInput::Key { scan_code, extended: extended != 0, down: down != 0 });
}

#[no_mangle]
pub extern "system" fn Java_com_remoteview_client_NativeBridge_nativeSendText<'l>(
    mut env: JNIEnv<'l>,
    _this: JObject<'l>,
    handle: jlong,
    text: JString<'l>,
) {
    if let Ok(s) = env.get_string(&text) {
        send_input(handle, UiInput::Text(s.into()));
    }
}

#[no_mangle]
pub extern "system" fn Java_com_remoteview_client_NativeBridge_nativeSendMouseAbs<'l>(
    _env: JNIEnv<'l>,
    _this: JObject<'l>,
    handle: jlong,
    nx: jfloat,
    ny: jfloat,
) {
    send_input(handle, UiInput::MouseAbs { nx, ny });
}

#[no_mangle]
pub extern "system" fn Java_com_remoteview_client_NativeBridge_nativeSendMouseRel<'l>(
    _env: JNIEnv<'l>,
    _this: JObject<'l>,
    handle: jlong,
    dx: jint,
    dy: jint,
) {
    send_input(handle, UiInput::MouseRel { dx, dy });
}

#[no_mangle]
pub extern "system" fn Java_com_remoteview_client_NativeBridge_nativeSendMouseButton<'l>(
    _env: JNIEnv<'l>,
    _this: JObject<'l>,
    handle: jlong,
    button: jint,
    down: jboolean,
) {
    send_input(handle, UiInput::MouseButton { button, down: down != 0 });
}

#[no_mangle]
pub extern "system" fn Java_com_remoteview_client_NativeBridge_nativeSendWheel<'l>(
    _env: JNIEnv<'l>,
    _this: JObject<'l>,
    handle: jlong,
    horizontal: jint,
    vertical: jint,
) {
    send_input(handle, UiInput::Wheel { horizontal, vertical });
}

#[no_mangle]
#[allow(clippy::too_many_arguments)]
pub extern "system" fn Java_com_remoteview_client_NativeBridge_nativeSendPen<'l>(
    _env: JNIEnv<'l>,
    _this: JObject<'l>,
    handle: jlong,
    nx: jfloat,
    ny: jfloat,
    pressure: jint,
    tilt_x: jint,
    tilt_y: jint,
    in_range: jboolean,
    in_contact: jboolean,
    barrel: jboolean,
    eraser: jboolean,
) {
    send_input(
        handle,
        UiInput::Pen {
            nx,
            ny,
            pressure,
            tilt_x,
            tilt_y,
            in_range: in_range != 0,
            in_contact: in_contact != 0,
            barrel: barrel != 0,
            eraser: eraser != 0,
        },
    );
}

#[no_mangle]
pub extern "system" fn Java_com_remoteview_client_NativeBridge_nativeSendSas<'l>(
    _env: JNIEnv<'l>,
    _this: JObject<'l>,
    handle: jlong,
) {
    send_input(handle, UiInput::Sas);
}

#[no_mangle]
pub extern "system" fn Java_com_remoteview_client_NativeBridge_nativeSendTouch<'l>(
    _env: JNIEnv<'l>,
    _this: JObject<'l>,
    handle: jlong,
    id: jint,
    x: jint,
    y: jint,
    phase: jint,
    pressure: jint,
) {
    send_input(handle, UiInput::Touch { id, x, y, phase, pressure });
}
