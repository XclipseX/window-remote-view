//! Pure `#[repr(C)]` FFI bindings for AMD Advanced Media Framework (AMF) runtime (`amfrt64.dll`).
//!
//! Zero external crate dependencies (no C/C++ AMF SDK headers or bindgen required).
//! All vtable layouts, struct layouts, GUIDs, result codes, and property constants
//! are verified against the upstream MIT-licensed GPUOpen AMF 1.4/1.5 C headers.
//!
//! Key Win64 (LLP64) ABI invariants:
//! - `amf_long` is `i32` (32-bit signed `long` on Windows x64, NOT `isize`).
//! - `amf_bool` is `u8`, `amf_pts` is `i64`, `amf_size` is `usize`.
//! - DLL entry points (`AMFQueryVersion`, `AMFInit`) use `extern "C"`.
//! - All AMF interface vtable methods use `extern "system"` (`__stdcall` on Win32 / Win64 ABI).
//! - `AMFFactory` is a non-refcounted singleton (slot 0 = `CreateContext`; NEVER call `Release`).
//! - `AMFVariantStruct` is 24 bytes, 8-byte aligned, passed BY VALUE to `SetProperty`.
//! - `AMFComponentVtbl` reflects MSVC's reverse-overload vtable order for `GetPropertyInfo` /
//!   `GetPropertyInfoAt` on Windows.

#![allow(non_camel_case_types, non_snake_case, dead_code)]

use std::ffi::{c_char, c_void};

// --------------------------------------------------------------------------
// Primitive AMF types (Win64 LLP64)
// --------------------------------------------------------------------------

pub type amf_long = i32;
pub type amf_int32 = i32;
pub type amf_uint32 = u32;
pub type amf_int64 = i64;
pub type amf_uint64 = u64;
pub type amf_uint16 = u16;
pub type amf_uint8 = u8;
pub type amf_bool = u8;
pub type amf_pts = i64;
pub type amf_size = usize;
pub type amf_float = f32;
pub type amf_double = f64;
pub type amf_handle = *mut c_void;
pub type AmfResult = i32;

// --------------------------------------------------------------------------
// Version helpers (`core/Version.h`)
// --------------------------------------------------------------------------

#[inline]
pub const fn amf_make_full_version(major: u64, minor: u64, release: u64, build: u64) -> amf_uint64 {
    ((major & 0xFFFF) << 48)
        | ((minor & 0xFFFF) << 32)
        | ((release & 0xFFFF) << 16)
        | (build & 0xFFFF)
}

#[inline]
pub const fn amf_get_major_version(ver: amf_uint64) -> u16 {
    ((ver >> 48) & 0xFFFF) as u16
}

#[inline]
pub const fn amf_get_minor_version(ver: amf_uint64) -> u16 {
    ((ver >> 32) & 0xFFFF) as u16
}

#[inline]
pub const fn amf_get_subminor_version(ver: amf_uint64) -> u16 {
    ((ver >> 16) & 0xFFFF) as u16
}

#[inline]
pub const fn amf_get_build_version(ver: amf_uint64) -> u16 {
    (ver & 0xFFFF) as u16
}

/// Format an AMF packed 64-bit version as `"major.minor.subminor"`.
pub fn format_amf_version(ver: amf_uint64) -> String {
    format!(
        "{}.{}.{}",
        amf_get_major_version(ver),
        amf_get_minor_version(ver),
        amf_get_subminor_version(ver),
    )
}

/// Minimum AMF API version requested when initializing (1.4.18.0).
pub const AMF_MIN_REQUEST_VERSION: amf_uint64 = amf_make_full_version(1, 4, 18, 0);
/// Maximum AMF API version cap passed to `AMFInit` (1.4.36.0).
pub const AMF_MAX_INIT_VERSION: amf_uint64 = amf_make_full_version(1, 4, 36, 0);

// --------------------------------------------------------------------------
// Wide string helper
// --------------------------------------------------------------------------

/// Encode a UTF-8 Rust `&str` as a NUL-terminated UTF-16 `Vec<u16>` (`const wchar_t*` on Windows).
pub fn wide_nul(s: &str) -> Vec<u16> {
    let mut v: Vec<u16> = s.encode_utf16().collect();
    v.push(0);
    v
}

// --------------------------------------------------------------------------
// AMF_RESULT codes (`core/Result.h`)
// --------------------------------------------------------------------------

pub const AMF_OK: AmfResult = 0;
pub const AMF_FAIL: AmfResult = 1;
pub const AMF_UNEXPECTED: AmfResult = 2;
pub const AMF_ACCESS_DENIED: AmfResult = 3;
pub const AMF_INVALID_ARG: AmfResult = 4;
pub const AMF_OUT_OF_RANGE: AmfResult = 5;
pub const AMF_OUT_OF_MEMORY: AmfResult = 6;
pub const AMF_INVALID_POINTER: AmfResult = 7;
pub const AMF_NO_INTERFACE: AmfResult = 8;
pub const AMF_NOT_IMPLEMENTED: AmfResult = 9;
pub const AMF_NOT_SUPPORTED: AmfResult = 10;
pub const AMF_NOT_FOUND: AmfResult = 11;
pub const AMF_ALREADY_INITIALIZED: AmfResult = 12;
pub const AMF_NOT_INITIALIZED: AmfResult = 13;
pub const AMF_INVALID_FORMAT: AmfResult = 14;
pub const AMF_WRONG_STATE: AmfResult = 15;
pub const AMF_FILE_NOT_OPEN: AmfResult = 16;
pub const AMF_NO_DEVICE: AmfResult = 17;
pub const AMF_DIRECTX_FAILED: AmfResult = 18;
pub const AMF_EOF: AmfResult = 23;
pub const AMF_REPEAT: AmfResult = 24;
pub const AMF_INPUT_FULL: AmfResult = 25;
pub const AMF_RESOLUTION_CHANGED: AmfResult = 26;
pub const AMF_RESOLUTION_UPDATED: AmfResult = 27;
pub const AMF_INVALID_DATA_TYPE: AmfResult = 28;
pub const AMF_INVALID_RESOLUTION: AmfResult = 29;
pub const AMF_CODEC_NOT_SUPPORTED: AmfResult = 30;
pub const AMF_SURFACE_FORMAT_NOT_SUPPORTED: AmfResult = 31;
pub const AMF_SURFACE_MUST_BE_SHARED: AmfResult = 32;
pub const AMF_ENCODER_NOT_PRESENT: AmfResult = 36;
pub const AMF_NEED_MORE_INPUT: AmfResult = 44;

pub fn result_name(r: AmfResult) -> &'static str {
    match r {
        AMF_OK => "AMF_OK",
        AMF_FAIL => "AMF_FAIL",
        AMF_UNEXPECTED => "AMF_UNEXPECTED",
        AMF_ACCESS_DENIED => "AMF_ACCESS_DENIED",
        AMF_INVALID_ARG => "AMF_INVALID_ARG",
        AMF_OUT_OF_RANGE => "AMF_OUT_OF_RANGE",
        AMF_OUT_OF_MEMORY => "AMF_OUT_OF_MEMORY",
        AMF_INVALID_POINTER => "AMF_INVALID_POINTER",
        AMF_NO_INTERFACE => "AMF_NO_INTERFACE",
        AMF_NOT_IMPLEMENTED => "AMF_NOT_IMPLEMENTED",
        AMF_NOT_SUPPORTED => "AMF_NOT_SUPPORTED",
        AMF_NOT_FOUND => "AMF_NOT_FOUND",
        AMF_ALREADY_INITIALIZED => "AMF_ALREADY_INITIALIZED",
        AMF_NOT_INITIALIZED => "AMF_NOT_INITIALIZED",
        AMF_INVALID_FORMAT => "AMF_INVALID_FORMAT",
        AMF_WRONG_STATE => "AMF_WRONG_STATE",
        AMF_FILE_NOT_OPEN => "AMF_FILE_NOT_OPEN",
        AMF_NO_DEVICE => "AMF_NO_DEVICE",
        AMF_DIRECTX_FAILED => "AMF_DIRECTX_FAILED",
        AMF_EOF => "AMF_EOF",
        AMF_REPEAT => "AMF_REPEAT",
        AMF_INPUT_FULL => "AMF_INPUT_FULL",
        AMF_RESOLUTION_CHANGED => "AMF_RESOLUTION_CHANGED",
        AMF_RESOLUTION_UPDATED => "AMF_RESOLUTION_UPDATED",
        AMF_INVALID_DATA_TYPE => "AMF_INVALID_DATA_TYPE",
        AMF_INVALID_RESOLUTION => "AMF_INVALID_RESOLUTION",
        AMF_CODEC_NOT_SUPPORTED => "AMF_CODEC_NOT_SUPPORTED",
        AMF_SURFACE_FORMAT_NOT_SUPPORTED => "AMF_SURFACE_FORMAT_NOT_SUPPORTED",
        AMF_SURFACE_MUST_BE_SHARED => "AMF_SURFACE_MUST_BE_SHARED",
        AMF_ENCODER_NOT_PRESENT => "AMF_ENCODER_NOT_PRESENT",
        AMF_NEED_MORE_INPUT => "AMF_NEED_MORE_INPUT",
        _ => "AMF_UNKNOWN_ERROR",
    }
}

// --------------------------------------------------------------------------
// Core enums (`core/Data.h`, `core/Surface.h`, `core/Variant.h`)
// --------------------------------------------------------------------------

pub type AMF_DX_VERSION = i32;
pub const AMF_DX9: AMF_DX_VERSION = 90;
pub const AMF_DX9_EX: AMF_DX_VERSION = 91;
pub const AMF_DX11_0: AMF_DX_VERSION = 110;
pub const AMF_DX11_1: AMF_DX_VERSION = 111;
pub const AMF_DX12: AMF_DX_VERSION = 120;

pub type AMF_MEMORY_TYPE = i32;
pub const AMF_MEMORY_UNKNOWN: AMF_MEMORY_TYPE = 0;
pub const AMF_MEMORY_HOST: AMF_MEMORY_TYPE = 1;
pub const AMF_MEMORY_DX9: AMF_MEMORY_TYPE = 2;
pub const AMF_MEMORY_DX11: AMF_MEMORY_TYPE = 3;
pub const AMF_MEMORY_OPENCL: AMF_MEMORY_TYPE = 4;
pub const AMF_MEMORY_OPENGL: AMF_MEMORY_TYPE = 5;
pub const AMF_MEMORY_VULKAN: AMF_MEMORY_TYPE = 10;
pub const AMF_MEMORY_DX12: AMF_MEMORY_TYPE = 11;

pub type AMF_DATA_TYPE = i32;
pub type AMF_FRAME_TYPE = i32;
pub type AMF_PLANE_TYPE = i32;
pub type AMF_AUDIO_FORMAT = i32;

pub type AMF_SURFACE_FORMAT = i32;
pub const AMF_SURFACE_UNKNOWN: AMF_SURFACE_FORMAT = 0;
pub const AMF_SURFACE_NV12: AMF_SURFACE_FORMAT = 1;
pub const AMF_SURFACE_YV12: AMF_SURFACE_FORMAT = 2;
pub const AMF_SURFACE_BGRA: AMF_SURFACE_FORMAT = 3;
pub const AMF_SURFACE_ARGB: AMF_SURFACE_FORMAT = 4;
pub const AMF_SURFACE_RGBA: AMF_SURFACE_FORMAT = 5;
pub const AMF_SURFACE_YUV420P: AMF_SURFACE_FORMAT = 7;
pub const AMF_SURFACE_P010: AMF_SURFACE_FORMAT = 10;

pub type AMF_VARIANT_TYPE = i32;
pub const AMF_VARIANT_EMPTY: AMF_VARIANT_TYPE = 0;
pub const AMF_VARIANT_BOOL: AMF_VARIANT_TYPE = 1;
pub const AMF_VARIANT_INT64: AMF_VARIANT_TYPE = 2;
pub const AMF_VARIANT_DOUBLE: AMF_VARIANT_TYPE = 3;
pub const AMF_VARIANT_RECT: AMF_VARIANT_TYPE = 4;
pub const AMF_VARIANT_SIZE: AMF_VARIANT_TYPE = 5;
pub const AMF_VARIANT_POINT: AMF_VARIANT_TYPE = 6;
pub const AMF_VARIANT_RATE: AMF_VARIANT_TYPE = 7;
pub const AMF_VARIANT_RATIO: AMF_VARIANT_TYPE = 8;
pub const AMF_VARIANT_COLOR: AMF_VARIANT_TYPE = 9;
pub const AMF_VARIANT_STRING: AMF_VARIANT_TYPE = 10;
pub const AMF_VARIANT_WSTRING: AMF_VARIANT_TYPE = 11;
pub const AMF_VARIANT_INTERFACE: AMF_VARIANT_TYPE = 12;
pub const AMF_VARIANT_FLOAT: AMF_VARIANT_TYPE = 13;

// --------------------------------------------------------------------------
// Geometry, GUID, and AMFVariantStruct (`core/Platform.h`, `core/Variant.h`)
// --------------------------------------------------------------------------

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AMFGuid {
    pub data1: amf_uint32,
    pub data2: amf_uint16,
    pub data3: amf_uint16,
    pub data4: [amf_uint8; 8],
}

/// `AMF_DECLARE_IID(AMFBuffer, 0xb04b7248, 0xb6f0, 0x4321, 0xb6, 0x91, 0xba, 0xa4, 0x74, 0xf, 0x9f, 0xcb)`
pub const IID_AMF_BUFFER: AMFGuid = AMFGuid {
    data1: 0xb04b_7248,
    data2: 0xb6f0,
    data3: 0x4321,
    data4: [0xb6, 0x91, 0xba, 0xa4, 0x74, 0x0f, 0x9f, 0xcb],
};

/// `AMF_DECLARE_IID(AMFInterface, 0x9d872f34, 0x90dc, 0x4b93, 0xb6, 0xb2, 0x6c, 0xa3, 0x7c, 0x85, 0x25, 0xdb)`
pub const IID_AMF_INTERFACE: AMFGuid = AMFGuid {
    data1: 0x9d87_2f34,
    data2: 0x90dc,
    data3: 0x4b93,
    data4: [0xb6, 0xb2, 0x6c, 0xa3, 0x7c, 0x85, 0x25, 0xdb],
};

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AMFRect {
    pub left: amf_int32,
    pub top: amf_int32,
    pub right: amf_int32,
    pub bottom: amf_int32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AMFSize {
    pub width: amf_int32,
    pub height: amf_int32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AMFPoint {
    pub x: amf_int32,
    pub y: amf_int32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AMFRate {
    pub num: amf_uint32,
    pub den: amf_uint32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AMFRatio {
    pub num: amf_uint32,
    pub den: amf_uint32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AMFColor {
    pub r: amf_uint8,
    pub g: amf_uint8,
    pub b: amf_uint8,
    pub a: amf_uint8,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct AMFFloatSize {
    pub width: amf_float,
    pub height: amf_float,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct AMFFloatPoint2D {
    pub x: amf_float,
    pub y: amf_float,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct AMFFloatPoint3D {
    pub x: amf_float,
    pub y: amf_float,
    pub z: amf_float,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct AMFFloatVector4D {
    pub x: amf_float,
    pub y: amf_float,
    pub z: amf_float,
    pub w: amf_float,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub union AMFVariantUnion {
    pub bool_value: amf_bool,
    pub int64_value: amf_int64,
    pub double_value: amf_double,
    pub string_value: *mut c_char,
    pub wstring_value: *mut u16,
    pub interface_value: *mut AMFInterface,
    pub rect_value: AMFRect,
    pub size_value: AMFSize,
    pub point_value: AMFPoint,
    pub rate_value: AMFRate,
    pub ratio_value: AMFRatio,
    pub color_value: AMFColor,
    pub float_value: amf_float,
    pub float_size_value: AMFFloatSize,
    pub float_point2d_value: AMFFloatPoint2D,
    pub float_point3d_value: AMFFloatPoint3D,
    pub float_vector4d_value: AMFFloatVector4D,
    /// Full 16-byte raw storage so zero-initialization clears all trailing bytes.
    pub raw: [u8; 16],
}

/// `AMFVariantStruct` (24 bytes, align 8 on 64-bit targets; passed by value to `SetProperty`).
#[repr(C)]
#[derive(Clone, Copy)]
pub struct AMFVariantStruct {
    pub variant_type: AMF_VARIANT_TYPE,
    pub value: AMFVariantUnion,
}

impl AMFVariantStruct {
    #[inline]
    pub const fn empty() -> Self {
        Self {
            variant_type: AMF_VARIANT_EMPTY,
            value: AMFVariantUnion { raw: [0u8; 16] },
        }
    }

    #[inline]
    pub fn from_bool(v: bool) -> Self {
        let mut s = Self::empty();
        s.variant_type = AMF_VARIANT_BOOL;
        s.value.bool_value = if v { 1 } else { 0 };
        s
    }

    #[inline]
    pub fn from_i64(v: i64) -> Self {
        let mut s = Self::empty();
        s.variant_type = AMF_VARIANT_INT64;
        s.value.int64_value = v;
        s
    }

    #[inline]
    pub fn from_size(width: i32, height: i32) -> Self {
        let mut s = Self::empty();
        s.variant_type = AMF_VARIANT_SIZE;
        s.value.size_value = AMFSize { width, height };
        s
    }

    #[inline]
    pub fn from_rate(num: u32, den: u32) -> Self {
        let mut s = Self::empty();
        s.variant_type = AMF_VARIANT_RATE;
        s.value.rate_value = AMFRate { num, den };
        s
    }
}

// --------------------------------------------------------------------------
// Encoder component IDs, property names, and enum values
// (`components/VideoEncoderHEVC.h` & `components/VideoEncoderVCE.h`)
// --------------------------------------------------------------------------

pub const AMF_COMPONENT_HEVC: &str = "AMFVideoEncoderHW_HEVC";
pub const AMF_COMPONENT_AVC: &str = "AMFVideoEncoderVCE_AVC";

// ---- HEVC (`VideoEncoderHEVC.h`) ----

pub const HEVC_USAGE: &str = "HevcUsage";
pub const HEVC_USAGE_TRANSCODING: i64 = 0;
pub const HEVC_USAGE_ULTRA_LOW_LATENCY: i64 = 1;
pub const HEVC_USAGE_LOW_LATENCY: i64 = 2;

pub const HEVC_LOW_LATENCY_MODE: &str = "LowLatencyInternal";

pub const HEVC_QUALITY_PRESET: &str = "HevcQualityPreset";
pub const HEVC_QUALITY_PRESET_QUALITY: i64 = 0;
pub const HEVC_QUALITY_PRESET_BALANCED: i64 = 5;
pub const HEVC_QUALITY_PRESET_SPEED: i64 = 10;

pub const HEVC_RATE_CONTROL_METHOD: &str = "HevcRateControlMethod";
pub const HEVC_RC_CONSTANT_QP: i64 = 0;
pub const HEVC_RC_LATENCY_CONSTRAINED_VBR: i64 = 1;
pub const HEVC_RC_PEAK_CONSTRAINED_VBR: i64 = 2;
pub const HEVC_RC_CBR: i64 = 3;

pub const HEVC_TARGET_BITRATE: &str = "HevcTargetBitrate";
pub const HEVC_PEAK_BITRATE: &str = "HevcPeakBitrate";
pub const HEVC_VBV_BUFFER_SIZE: &str = "HevcVBVBufferSize";
pub const HEVC_FRAME_RATE: &str = "HevcFrameRate";
pub const HEVC_FRAME_SIZE: &str = "HevcFrameSize";

pub const HEVC_PROFILE: &str = "HevcProfile";
pub const HEVC_PROFILE_MAIN: i64 = 1;
pub const HEVC_PROFILE_MAIN_10: i64 = 2;

pub const HEVC_HEADER_INSERTION_MODE: &str = "HevcHeaderInsertionMode";
pub const HEVC_HEADER_INSERTION_NONE: i64 = 0;
pub const HEVC_HEADER_INSERTION_GOP_ALIGNED: i64 = 1;
pub const HEVC_HEADER_INSERTION_IDR_ALIGNED: i64 = 2;

pub const HEVC_GOP_SIZE: &str = "HevcGOPSize";
pub const HEVC_GOPS_PER_IDR: &str = "HevcGOPSPerIDR";
pub const HEVC_FILLER_DATA_ENABLE: &str = "HevcFillerDataEnable";
pub const HEVC_RATE_CONTROL_PREANALYSIS_ENABLE: &str = "HevcRateControlPreAnalysisEnable";
pub const HEVC_PRE_ANALYSIS_ENABLE: &str = "HevcEnablePreAnalysis";
pub const HEVC_ENABLE_VBAQ: &str = "HevcEnableVBAQ";
pub const HEVC_QUERY_TIMEOUT: &str = "HevcQueryTimeout";
pub const HEVC_ENGINE_TYPE: &str = "HevcEngineType";
pub const HEVC_NOMINAL_RANGE: &str = "HevcNominalRange";
pub const HEVC_NOMINAL_RANGE_STUDIO: i64 = 0;
pub const HEVC_NOMINAL_RANGE_FULL: i64 = 1;

// Per-surface HEVC properties
pub const HEVC_FORCE_PICTURE_TYPE: &str = "HevcForcePictureType";
pub const HEVC_PICTURE_TYPE_NONE: i64 = 0;
pub const HEVC_PICTURE_TYPE_SKIP: i64 = 1;
pub const HEVC_PICTURE_TYPE_IDR: i64 = 2;
pub const HEVC_INSERT_HEADER: &str = "HevcInsertHeader";

// ---- AVC / H.264 (`VideoEncoderVCE.h`) ----

pub const AVC_USAGE: &str = "Usage";
pub const AVC_USAGE_TRANSCODING: i64 = 0;
pub const AVC_USAGE_ULTRA_LOW_LATENCY: i64 = 1;
pub const AVC_USAGE_LOW_LATENCY: i64 = 2;

pub const AVC_LOW_LATENCY_MODE: &str = "LowLatencyInternal";

pub const AVC_QUALITY_PRESET: &str = "QualityPreset";
pub const AVC_QUALITY_PRESET_BALANCED: i64 = 0;
pub const AVC_QUALITY_PRESET_SPEED: i64 = 1;
pub const AVC_QUALITY_PRESET_QUALITY: i64 = 2;

pub const AVC_RATE_CONTROL_METHOD: &str = "RateControlMethod";
pub const AVC_RC_CONSTANT_QP: i64 = 0;
pub const AVC_RC_CBR: i64 = 1;
pub const AVC_RC_PEAK_CONSTRAINED_VBR: i64 = 2;
pub const AVC_RC_LATENCY_CONSTRAINED_VBR: i64 = 3;

pub const AVC_TARGET_BITRATE: &str = "TargetBitrate";
pub const AVC_PEAK_BITRATE: &str = "PeakBitrate";
pub const AVC_VBV_BUFFER_SIZE: &str = "VBVBufferSize";
pub const AVC_FRAME_RATE: &str = "FrameRate";
pub const AVC_FRAME_SIZE: &str = "FrameSize";

pub const AVC_PROFILE: &str = "Profile";
pub const AVC_PROFILE_BASELINE: i64 = 66;
pub const AVC_PROFILE_MAIN: i64 = 77;
pub const AVC_PROFILE_HIGH: i64 = 100;

pub const AVC_MAX_CONSECUTIVE_BPICTURES: &str = "MaxConsecutiveBPictures";
pub const AVC_B_PICTURES_PATTERN: &str = "BPicturesPattern";
pub const AVC_IDR_PERIOD: &str = "IDRPeriod";
pub const AVC_FILLER_DATA_ENABLE: &str = "FillerDataEnable";
/// Note: upstream `VideoEncoderVCE.h` spells this `L"RateControlPreanalysisEnable"` (lowercase `a`)
/// with type `amf_int64` (`0` = disabled, `1` = enabled).
pub const AVC_RATE_CONTROL_PREANALYSIS_ENABLE: &str = "RateControlPreanalysisEnable";
pub const AVC_PREENCODE_DISABLED: i64 = 0;
pub const AVC_PRE_ANALYSIS_ENABLE: &str = "EnablePreAnalysis";
pub const AVC_ENABLE_VBAQ: &str = "EnableVBAQ";
pub const AVC_QUERY_TIMEOUT: &str = "QueryTimeout";
pub const AVC_ENGINE_TYPE: &str = "EngineType";

// Per-surface AVC properties
pub const AVC_FORCE_PICTURE_TYPE: &str = "ForcePictureType";
pub const AVC_PICTURE_TYPE_NONE: i64 = 0;
pub const AVC_PICTURE_TYPE_SKIP: i64 = 1;
pub const AVC_PICTURE_TYPE_IDR: i64 = 2;
pub const AVC_INSERT_SPS: &str = "InsertSPS";
pub const AVC_INSERT_PPS: &str = "InsertPPS";

// --------------------------------------------------------------------------
// Opaque placeholder types for unused vtable signatures
// --------------------------------------------------------------------------

#[repr(C)]
pub struct AMFPropertyStorage {
    pub pVtbl: *const c_void,
}
#[repr(C)]
pub struct AMFPropertyStorageObserver {
    _opaque: [u8; 0],
}
#[repr(C)]
pub struct AMFPropertyInfo {
    _opaque: [u8; 0],
}
#[repr(C)]
pub struct AMFDebug {
    _opaque: [u8; 0],
}
#[repr(C)]
pub struct AMFTrace {
    _opaque: [u8; 0],
}
#[repr(C)]
pub struct AMFPrograms {
    _opaque: [u8; 0],
}
#[repr(C)]
pub struct AMFComputeFactory {
    _opaque: [u8; 0],
}
#[repr(C)]
pub struct AMFComputeDevice {
    _opaque: [u8; 0],
}
#[repr(C)]
pub struct AMFCompute {
    _opaque: [u8; 0],
}
#[repr(C)]
pub struct AMFAudioBuffer {
    _opaque: [u8; 0],
}
#[repr(C)]
pub struct AMFBufferObserver {
    _opaque: [u8; 0],
}
#[repr(C)]
pub struct AMFSurfaceObserver {
    _opaque: [u8; 0],
}
#[repr(C)]
pub struct AMFPlane {
    _opaque: [u8; 0],
}
#[repr(C)]
pub struct AMFDataAllocatorCB {
    _opaque: [u8; 0],
}
#[repr(C)]
pub struct AMFCaps {
    _opaque: [u8; 0],
}
#[repr(C)]
pub struct AMFComponentOptimizationCallback {
    _opaque: [u8; 0],
}

// --------------------------------------------------------------------------
// Interface structs & exact C vtables
// --------------------------------------------------------------------------

#[repr(C)]
pub struct AMFInterface {
    pub pVtbl: *const AMFInterfaceVtbl,
}

#[repr(C)]
pub struct AMFInterfaceVtbl {
    pub Acquire: unsafe extern "system" fn(pThis: *mut AMFInterface) -> amf_long,
    pub Release: unsafe extern "system" fn(pThis: *mut AMFInterface) -> amf_long,
    pub QueryInterface: unsafe extern "system" fn(
        pThis: *mut AMFInterface,
        interfaceID: *const AMFGuid,
        ppInterface: *mut *mut c_void,
    ) -> AmfResult,
}

/// `AMFFactory` is NOT refcounted — slot 0 is `CreateContext`. NEVER call `Release`.
#[repr(C)]
pub struct AMFFactory {
    pub pVtbl: *const AMFFactoryVtbl,
}

#[repr(C)]
pub struct AMFFactoryVtbl {
    pub CreateContext: unsafe extern "system" fn(
        pThis: *mut AMFFactory,
        ppContext: *mut *mut AMFContext,
    ) -> AmfResult,
    pub CreateComponent: unsafe extern "system" fn(
        pThis: *mut AMFFactory,
        pContext: *mut AMFContext,
        id: *const u16,
        ppComponent: *mut *mut AMFComponent,
    ) -> AmfResult,
    pub SetCacheFolder:
        unsafe extern "system" fn(pThis: *mut AMFFactory, path: *const u16) -> AmfResult,
    pub GetCacheFolder: unsafe extern "system" fn(pThis: *mut AMFFactory) -> *const u16,
    pub GetDebug: unsafe extern "system" fn(
        pThis: *mut AMFFactory,
        ppDebug: *mut *mut AMFDebug,
    ) -> AmfResult,
    pub GetTrace: unsafe extern "system" fn(
        pThis: *mut AMFFactory,
        ppTrace: *mut *mut AMFTrace,
    ) -> AmfResult,
    pub GetPrograms: unsafe extern "system" fn(
        pThis: *mut AMFFactory,
        ppPrograms: *mut *mut AMFPrograms,
    ) -> AmfResult,
}

#[repr(C)]
pub struct AMFContext {
    pub pVtbl: *const AMFContextVtbl,
}

#[repr(C)]
pub struct AMFContextVtbl {
    // AMFInterface (slots 0..=2)
    pub Acquire: unsafe extern "system" fn(pThis: *mut AMFContext) -> amf_long,
    pub Release: unsafe extern "system" fn(pThis: *mut AMFContext) -> amf_long,
    pub QueryInterface: unsafe extern "system" fn(
        pThis: *mut AMFContext,
        interfaceID: *const AMFGuid,
        ppInterface: *mut *mut c_void,
    ) -> AmfResult,

    // AMFPropertyStorage (slots 3..=12)
    pub SetProperty: unsafe extern "system" fn(
        pThis: *mut AMFContext,
        name: *const u16,
        value: AMFVariantStruct,
    ) -> AmfResult,
    pub GetProperty: unsafe extern "system" fn(
        pThis: *mut AMFContext,
        name: *const u16,
        pValue: *mut AMFVariantStruct,
    ) -> AmfResult,
    pub HasProperty:
        unsafe extern "system" fn(pThis: *mut AMFContext, name: *const u16) -> amf_bool,
    pub GetPropertyCount: unsafe extern "system" fn(pThis: *mut AMFContext) -> amf_size,
    pub GetPropertyAt: unsafe extern "system" fn(
        pThis: *mut AMFContext,
        index: amf_size,
        name: *mut u16,
        nameSize: amf_size,
        pValue: *mut AMFVariantStruct,
    ) -> AmfResult,
    pub Clear: unsafe extern "system" fn(pThis: *mut AMFContext) -> AmfResult,
    pub AddTo: unsafe extern "system" fn(
        pThis: *mut AMFContext,
        pDest: *mut AMFPropertyStorage,
        overwrite: amf_bool,
        deep: amf_bool,
    ) -> AmfResult,
    pub CopyTo: unsafe extern "system" fn(
        pThis: *mut AMFContext,
        pDest: *mut AMFPropertyStorage,
        deep: amf_bool,
    ) -> AmfResult,
    pub AddObserver: unsafe extern "system" fn(
        pThis: *mut AMFContext,
        pObserver: *mut AMFPropertyStorageObserver,
    ),
    pub RemoveObserver: unsafe extern "system" fn(
        pThis: *mut AMFContext,
        pObserver: *mut AMFPropertyStorageObserver,
    ),

    // AMFContext (slots 13..=54)
    pub Terminate: unsafe extern "system" fn(pThis: *mut AMFContext) -> AmfResult,

    // DX9 (14..=17)
    pub InitDX9:
        unsafe extern "system" fn(pThis: *mut AMFContext, pDX9Device: *mut c_void) -> AmfResult,
    pub GetDX9Device: unsafe extern "system" fn(
        pThis: *mut AMFContext,
        dxVersionRequired: AMF_DX_VERSION,
    ) -> *mut c_void,
    pub LockDX9: unsafe extern "system" fn(pThis: *mut AMFContext) -> AmfResult,
    pub UnlockDX9: unsafe extern "system" fn(pThis: *mut AMFContext) -> AmfResult,

    // DX11 (18..=21)
    pub InitDX11: unsafe extern "system" fn(
        pThis: *mut AMFContext,
        pDX11Device: *mut c_void,
        dxVersionRequired: AMF_DX_VERSION,
    ) -> AmfResult,
    pub GetDX11Device: unsafe extern "system" fn(
        pThis: *mut AMFContext,
        dxVersionRequired: AMF_DX_VERSION,
    ) -> *mut c_void,
    pub LockDX11: unsafe extern "system" fn(pThis: *mut AMFContext) -> AmfResult,
    pub UnlockDX11: unsafe extern "system" fn(pThis: *mut AMFContext) -> AmfResult,

    // OpenCL (22..=29)
    pub InitOpenCL:
        unsafe extern "system" fn(pThis: *mut AMFContext, pCommandQueue: *mut c_void) -> AmfResult,
    pub GetOpenCLContext: unsafe extern "system" fn(pThis: *mut AMFContext) -> *mut c_void,
    pub GetOpenCLCommandQueue: unsafe extern "system" fn(pThis: *mut AMFContext) -> *mut c_void,
    pub GetOpenCLDeviceID: unsafe extern "system" fn(pThis: *mut AMFContext) -> *mut c_void,
    pub GetOpenCLComputeFactory: unsafe extern "system" fn(
        pThis: *mut AMFContext,
        ppFactory: *mut *mut AMFComputeFactory,
    ) -> AmfResult,
    pub InitOpenCLEx: unsafe extern "system" fn(
        pThis: *mut AMFContext,
        pDevice: *mut AMFComputeDevice,
    ) -> AmfResult,
    pub LockOpenCL: unsafe extern "system" fn(pThis: *mut AMFContext) -> AmfResult,
    pub UnlockOpenCL: unsafe extern "system" fn(pThis: *mut AMFContext) -> AmfResult,

    // OpenGL (30..=34)
    pub InitOpenGL: unsafe extern "system" fn(
        pThis: *mut AMFContext,
        hOpenGLContext: amf_handle,
        hWindow: amf_handle,
        hDC: amf_handle,
    ) -> AmfResult,
    pub GetOpenGLContext: unsafe extern "system" fn(pThis: *mut AMFContext) -> amf_handle,
    pub GetOpenGLDrawable: unsafe extern "system" fn(pThis: *mut AMFContext) -> amf_handle,
    pub LockOpenGL: unsafe extern "system" fn(pThis: *mut AMFContext) -> AmfResult,
    pub UnlockOpenGL: unsafe extern "system" fn(pThis: *mut AMFContext) -> AmfResult,

    // XV - Linux (35..=38)
    pub InitXV:
        unsafe extern "system" fn(pThis: *mut AMFContext, pXVDevice: *mut c_void) -> AmfResult,
    pub GetXVDevice: unsafe extern "system" fn(pThis: *mut AMFContext) -> *mut c_void,
    pub LockXV: unsafe extern "system" fn(pThis: *mut AMFContext) -> AmfResult,
    pub UnlockXV: unsafe extern "system" fn(pThis: *mut AMFContext) -> AmfResult,

    // Gralloc - Android (39..=42)
    pub InitGralloc:
        unsafe extern "system" fn(pThis: *mut AMFContext, pGrallocDevice: *mut c_void) -> AmfResult,
    pub GetGrallocDevice: unsafe extern "system" fn(pThis: *mut AMFContext) -> *mut c_void,
    pub LockGralloc: unsafe extern "system" fn(pThis: *mut AMFContext) -> AmfResult,
    pub UnlockGralloc: unsafe extern "system" fn(pThis: *mut AMFContext) -> AmfResult,

    // Allocation (43..=45)
    pub AllocBuffer: unsafe extern "system" fn(
        pThis: *mut AMFContext,
        memType: AMF_MEMORY_TYPE,
        size: amf_size,
        ppBuffer: *mut *mut AMFBuffer,
    ) -> AmfResult,
    pub AllocSurface: unsafe extern "system" fn(
        pThis: *mut AMFContext,
        memType: AMF_MEMORY_TYPE,
        format: AMF_SURFACE_FORMAT,
        width: amf_int32,
        height: amf_int32,
        ppSurface: *mut *mut AMFSurface,
    ) -> AmfResult,
    pub AllocAudioBuffer: unsafe extern "system" fn(
        pThis: *mut AMFContext,
        memType: AMF_MEMORY_TYPE,
        format: AMF_AUDIO_FORMAT,
        samples: amf_int32,
        sampleRate: amf_int32,
        channels: amf_int32,
        ppAudioBuffer: *mut *mut AMFAudioBuffer,
    ) -> AmfResult,

    // Wrap existing native objects (46..=53)
    pub CreateBufferFromHostNative: unsafe extern "system" fn(
        pThis: *mut AMFContext,
        pHostBuffer: *mut c_void,
        size: amf_size,
        ppBuffer: *mut *mut AMFBuffer,
        pObserver: *mut AMFBufferObserver,
    ) -> AmfResult,
    pub CreateSurfaceFromHostNative: unsafe extern "system" fn(
        pThis: *mut AMFContext,
        format: AMF_SURFACE_FORMAT,
        width: amf_int32,
        height: amf_int32,
        hPitch: amf_int32,
        vPitch: amf_int32,
        pData: *mut c_void,
        ppSurface: *mut *mut AMFSurface,
        pObserver: *mut AMFSurfaceObserver,
    ) -> AmfResult,
    pub CreateSurfaceFromDX9Native: unsafe extern "system" fn(
        pThis: *mut AMFContext,
        pDX9Surface: *mut c_void,
        ppSurface: *mut *mut AMFSurface,
        pObserver: *mut AMFSurfaceObserver,
    ) -> AmfResult,
    pub CreateSurfaceFromDX11Native: unsafe extern "system" fn(
        pThis: *mut AMFContext,
        pDX11Surface: *mut c_void,
        ppSurface: *mut *mut AMFSurface,
        pObserver: *mut AMFSurfaceObserver,
    ) -> AmfResult,
    pub CreateSurfaceFromOpenGLNative: unsafe extern "system" fn(
        pThis: *mut AMFContext,
        format: AMF_SURFACE_FORMAT,
        hGLTextureID: amf_handle,
        ppSurface: *mut *mut AMFSurface,
        pObserver: *mut AMFSurfaceObserver,
    ) -> AmfResult,
    pub CreateSurfaceFromGrallocNative: unsafe extern "system" fn(
        pThis: *mut AMFContext,
        hGrallocSurface: amf_handle,
        ppSurface: *mut *mut AMFSurface,
        pObserver: *mut AMFSurfaceObserver,
    ) -> AmfResult,
    pub CreateSurfaceFromOpenCLNative: unsafe extern "system" fn(
        pThis: *mut AMFContext,
        format: AMF_SURFACE_FORMAT,
        width: amf_int32,
        height: amf_int32,
        pClPlanes: *mut *mut c_void,
        ppSurface: *mut *mut AMFSurface,
        pObserver: *mut AMFSurfaceObserver,
    ) -> AmfResult,
    pub CreateBufferFromOpenCLNative: unsafe extern "system" fn(
        pThis: *mut AMFContext,
        pCLBuffer: *mut c_void,
        size: amf_size,
        ppBuffer: *mut *mut AMFBuffer,
    ) -> AmfResult,

    // Compute (54)
    pub GetCompute: unsafe extern "system" fn(
        pThis: *mut AMFContext,
        eMemType: AMF_MEMORY_TYPE,
        ppCompute: *mut *mut AMFCompute,
    ) -> AmfResult,
}

#[repr(C)]
pub struct AMFComponent {
    pub pVtbl: *const AMFComponentVtbl,
}

#[repr(C)]
pub struct AMFComponentVtbl {
    // AMFInterface (slots 0..=2)
    pub Acquire: unsafe extern "system" fn(pThis: *mut AMFComponent) -> amf_long,
    pub Release: unsafe extern "system" fn(pThis: *mut AMFComponent) -> amf_long,
    pub QueryInterface: unsafe extern "system" fn(
        pThis: *mut AMFComponent,
        interfaceID: *const AMFGuid,
        ppInterface: *mut *mut c_void,
    ) -> AmfResult,

    // AMFPropertyStorage (slots 3..=12)
    pub SetProperty: unsafe extern "system" fn(
        pThis: *mut AMFComponent,
        name: *const u16,
        value: AMFVariantStruct,
    ) -> AmfResult,
    pub GetProperty: unsafe extern "system" fn(
        pThis: *mut AMFComponent,
        name: *const u16,
        pValue: *mut AMFVariantStruct,
    ) -> AmfResult,
    pub HasProperty:
        unsafe extern "system" fn(pThis: *mut AMFComponent, name: *const u16) -> amf_bool,
    pub GetPropertyCount: unsafe extern "system" fn(pThis: *mut AMFComponent) -> amf_size,
    pub GetPropertyAt: unsafe extern "system" fn(
        pThis: *mut AMFComponent,
        index: amf_size,
        name: *mut u16,
        nameSize: amf_size,
        pValue: *mut AMFVariantStruct,
    ) -> AmfResult,
    pub Clear: unsafe extern "system" fn(pThis: *mut AMFComponent) -> AmfResult,
    pub AddTo: unsafe extern "system" fn(
        pThis: *mut AMFComponent,
        pDest: *mut AMFPropertyStorage,
        overwrite: amf_bool,
        deep: amf_bool,
    ) -> AmfResult,
    pub CopyTo: unsafe extern "system" fn(
        pThis: *mut AMFComponent,
        pDest: *mut AMFPropertyStorage,
        deep: amf_bool,
    ) -> AmfResult,
    pub AddObserver: unsafe extern "system" fn(
        pThis: *mut AMFComponent,
        pObserver: *mut AMFPropertyStorageObserver,
    ),
    pub RemoveObserver: unsafe extern "system" fn(
        pThis: *mut AMFComponent,
        pObserver: *mut AMFPropertyStorageObserver,
    ),

    // AMFPropertyStorageEx (slots 13..=16; Windows MSVC reverses overloaded virtuals)
    pub GetPropertiesInfoCount: unsafe extern "system" fn(pThis: *mut AMFComponent) -> amf_size,
    pub GetPropertyInfo: unsafe extern "system" fn(
        pThis: *mut AMFComponent,
        name: *const u16,
        ppInfo: *mut *const AMFPropertyInfo,
    ) -> AmfResult,
    pub GetPropertyInfoAt: unsafe extern "system" fn(
        pThis: *mut AMFComponent,
        index: amf_size,
        ppInfo: *mut *const AMFPropertyInfo,
    ) -> AmfResult,
    pub ValidateProperty: unsafe extern "system" fn(
        pThis: *mut AMFComponent,
        name: *const u16,
        value: AMFVariantStruct,
        pOutValidated: *mut AMFVariantStruct,
    ) -> AmfResult,

    // AMFComponent (slots 17..=27)
    pub Init: unsafe extern "system" fn(
        pThis: *mut AMFComponent,
        format: AMF_SURFACE_FORMAT,
        width: amf_int32,
        height: amf_int32,
    ) -> AmfResult,
    pub ReInit: unsafe extern "system" fn(
        pThis: *mut AMFComponent,
        width: amf_int32,
        height: amf_int32,
    ) -> AmfResult,
    pub Terminate: unsafe extern "system" fn(pThis: *mut AMFComponent) -> AmfResult,
    pub Drain: unsafe extern "system" fn(pThis: *mut AMFComponent) -> AmfResult,
    pub Flush: unsafe extern "system" fn(pThis: *mut AMFComponent) -> AmfResult,
    pub SubmitInput:
        unsafe extern "system" fn(pThis: *mut AMFComponent, pData: *mut AMFData) -> AmfResult,
    pub QueryOutput: unsafe extern "system" fn(
        pThis: *mut AMFComponent,
        ppData: *mut *mut AMFData,
    ) -> AmfResult,
    pub GetContext: unsafe extern "system" fn(pThis: *mut AMFComponent) -> *mut AMFContext,
    pub SetOutputDataAllocatorCB: unsafe extern "system" fn(
        pThis: *mut AMFComponent,
        callback: *mut AMFDataAllocatorCB,
    ) -> AmfResult,
    pub GetCaps: unsafe extern "system" fn(
        pThis: *mut AMFComponent,
        ppCaps: *mut *mut AMFCaps,
    ) -> AmfResult,
    pub Optimize: unsafe extern "system" fn(
        pThis: *mut AMFComponent,
        pCallback: *mut AMFComponentOptimizationCallback,
    ) -> AmfResult,
}

#[repr(C)]
pub struct AMFData {
    pub pVtbl: *const AMFDataVtbl,
}

#[repr(C)]
pub struct AMFDataVtbl {
    // AMFInterface (slots 0..=2)
    pub Acquire: unsafe extern "system" fn(pThis: *mut AMFData) -> amf_long,
    pub Release: unsafe extern "system" fn(pThis: *mut AMFData) -> amf_long,
    pub QueryInterface: unsafe extern "system" fn(
        pThis: *mut AMFData,
        interfaceID: *const AMFGuid,
        ppInterface: *mut *mut c_void,
    ) -> AmfResult,

    // AMFPropertyStorage (slots 3..=12)
    pub SetProperty: unsafe extern "system" fn(
        pThis: *mut AMFData,
        name: *const u16,
        value: AMFVariantStruct,
    ) -> AmfResult,
    pub GetProperty: unsafe extern "system" fn(
        pThis: *mut AMFData,
        name: *const u16,
        pValue: *mut AMFVariantStruct,
    ) -> AmfResult,
    pub HasProperty: unsafe extern "system" fn(pThis: *mut AMFData, name: *const u16) -> amf_bool,
    pub GetPropertyCount: unsafe extern "system" fn(pThis: *mut AMFData) -> amf_size,
    pub GetPropertyAt: unsafe extern "system" fn(
        pThis: *mut AMFData,
        index: amf_size,
        name: *mut u16,
        nameSize: amf_size,
        pValue: *mut AMFVariantStruct,
    ) -> AmfResult,
    pub Clear: unsafe extern "system" fn(pThis: *mut AMFData) -> AmfResult,
    pub AddTo: unsafe extern "system" fn(
        pThis: *mut AMFData,
        pDest: *mut AMFPropertyStorage,
        overwrite: amf_bool,
        deep: amf_bool,
    ) -> AmfResult,
    pub CopyTo: unsafe extern "system" fn(
        pThis: *mut AMFData,
        pDest: *mut AMFPropertyStorage,
        deep: amf_bool,
    ) -> AmfResult,
    pub AddObserver: unsafe extern "system" fn(
        pThis: *mut AMFData,
        pObserver: *mut AMFPropertyStorageObserver,
    ),
    pub RemoveObserver: unsafe extern "system" fn(
        pThis: *mut AMFData,
        pObserver: *mut AMFPropertyStorageObserver,
    ),

    // AMFData (slots 13..=22)
    pub GetMemoryType: unsafe extern "system" fn(pThis: *mut AMFData) -> AMF_MEMORY_TYPE,
    pub Duplicate: unsafe extern "system" fn(
        pThis: *mut AMFData,
        memType: AMF_MEMORY_TYPE,
        ppData: *mut *mut AMFData,
    ) -> AmfResult,
    pub Convert:
        unsafe extern "system" fn(pThis: *mut AMFData, memType: AMF_MEMORY_TYPE) -> AmfResult,
    pub Interop:
        unsafe extern "system" fn(pThis: *mut AMFData, memType: AMF_MEMORY_TYPE) -> AmfResult,
    pub GetDataType: unsafe extern "system" fn(pThis: *mut AMFData) -> AMF_DATA_TYPE,
    pub IsReusable: unsafe extern "system" fn(pThis: *mut AMFData) -> amf_bool,
    pub SetPts: unsafe extern "system" fn(pThis: *mut AMFData, pts: amf_pts),
    pub GetPts: unsafe extern "system" fn(pThis: *mut AMFData) -> amf_pts,
    pub SetDuration: unsafe extern "system" fn(pThis: *mut AMFData, duration: amf_pts),
    pub GetDuration: unsafe extern "system" fn(pThis: *mut AMFData) -> amf_pts,
}

#[repr(C)]
pub struct AMFBuffer {
    pub pVtbl: *const AMFBufferVtbl,
}

#[repr(C)]
pub struct AMFBufferVtbl {
    // AMFInterface (slots 0..=2)
    pub Acquire: unsafe extern "system" fn(pThis: *mut AMFBuffer) -> amf_long,
    pub Release: unsafe extern "system" fn(pThis: *mut AMFBuffer) -> amf_long,
    pub QueryInterface: unsafe extern "system" fn(
        pThis: *mut AMFBuffer,
        interfaceID: *const AMFGuid,
        ppInterface: *mut *mut c_void,
    ) -> AmfResult,

    // AMFPropertyStorage (slots 3..=12)
    pub SetProperty: unsafe extern "system" fn(
        pThis: *mut AMFBuffer,
        name: *const u16,
        value: AMFVariantStruct,
    ) -> AmfResult,
    pub GetProperty: unsafe extern "system" fn(
        pThis: *mut AMFBuffer,
        name: *const u16,
        pValue: *mut AMFVariantStruct,
    ) -> AmfResult,
    pub HasProperty:
        unsafe extern "system" fn(pThis: *mut AMFBuffer, name: *const u16) -> amf_bool,
    pub GetPropertyCount: unsafe extern "system" fn(pThis: *mut AMFBuffer) -> amf_size,
    pub GetPropertyAt: unsafe extern "system" fn(
        pThis: *mut AMFBuffer,
        index: amf_size,
        name: *mut u16,
        nameSize: amf_size,
        pValue: *mut AMFVariantStruct,
    ) -> AmfResult,
    pub Clear: unsafe extern "system" fn(pThis: *mut AMFBuffer) -> AmfResult,
    pub AddTo: unsafe extern "system" fn(
        pThis: *mut AMFBuffer,
        pDest: *mut AMFPropertyStorage,
        overwrite: amf_bool,
        deep: amf_bool,
    ) -> AmfResult,
    pub CopyTo: unsafe extern "system" fn(
        pThis: *mut AMFBuffer,
        pDest: *mut AMFPropertyStorage,
        deep: amf_bool,
    ) -> AmfResult,
    pub AddObserver: unsafe extern "system" fn(
        pThis: *mut AMFBuffer,
        pObserver: *mut AMFPropertyStorageObserver,
    ),
    pub RemoveObserver: unsafe extern "system" fn(
        pThis: *mut AMFBuffer,
        pObserver: *mut AMFPropertyStorageObserver,
    ),

    // AMFData (slots 13..=22)
    pub GetMemoryType: unsafe extern "system" fn(pThis: *mut AMFBuffer) -> AMF_MEMORY_TYPE,
    pub Duplicate: unsafe extern "system" fn(
        pThis: *mut AMFBuffer,
        memType: AMF_MEMORY_TYPE,
        ppData: *mut *mut AMFData,
    ) -> AmfResult,
    pub Convert:
        unsafe extern "system" fn(pThis: *mut AMFBuffer, memType: AMF_MEMORY_TYPE) -> AmfResult,
    pub Interop:
        unsafe extern "system" fn(pThis: *mut AMFBuffer, memType: AMF_MEMORY_TYPE) -> AmfResult,
    pub GetDataType: unsafe extern "system" fn(pThis: *mut AMFBuffer) -> AMF_DATA_TYPE,
    pub IsReusable: unsafe extern "system" fn(pThis: *mut AMFBuffer) -> amf_bool,
    pub SetPts: unsafe extern "system" fn(pThis: *mut AMFBuffer, pts: amf_pts),
    pub GetPts: unsafe extern "system" fn(pThis: *mut AMFBuffer) -> amf_pts,
    pub SetDuration: unsafe extern "system" fn(pThis: *mut AMFBuffer, duration: amf_pts),
    pub GetDuration: unsafe extern "system" fn(pThis: *mut AMFBuffer) -> amf_pts,

    // AMFBuffer (slots 23..=27)
    pub SetSize: unsafe extern "system" fn(pThis: *mut AMFBuffer, newSize: amf_size) -> AmfResult,
    pub GetSize: unsafe extern "system" fn(pThis: *mut AMFBuffer) -> amf_size,
    pub GetNative: unsafe extern "system" fn(pThis: *mut AMFBuffer) -> *mut c_void,
    pub AddObserver_Buffer:
        unsafe extern "system" fn(pThis: *mut AMFBuffer, pObserver: *mut AMFBufferObserver),
    pub RemoveObserver_Buffer:
        unsafe extern "system" fn(pThis: *mut AMFBuffer, pObserver: *mut AMFBufferObserver),
}

#[repr(C)]
pub struct AMFSurface {
    pub pVtbl: *const AMFSurfaceVtbl,
}

#[repr(C)]
pub struct AMFSurfaceVtbl {
    // AMFInterface (slots 0..=2)
    pub Acquire: unsafe extern "system" fn(pThis: *mut AMFSurface) -> amf_long,
    pub Release: unsafe extern "system" fn(pThis: *mut AMFSurface) -> amf_long,
    pub QueryInterface: unsafe extern "system" fn(
        pThis: *mut AMFSurface,
        interfaceID: *const AMFGuid,
        ppInterface: *mut *mut c_void,
    ) -> AmfResult,

    // AMFPropertyStorage (slots 3..=12)
    pub SetProperty: unsafe extern "system" fn(
        pThis: *mut AMFSurface,
        name: *const u16,
        value: AMFVariantStruct,
    ) -> AmfResult,
    pub GetProperty: unsafe extern "system" fn(
        pThis: *mut AMFSurface,
        name: *const u16,
        pValue: *mut AMFVariantStruct,
    ) -> AmfResult,
    pub HasProperty:
        unsafe extern "system" fn(pThis: *mut AMFSurface, name: *const u16) -> amf_bool,
    pub GetPropertyCount: unsafe extern "system" fn(pThis: *mut AMFSurface) -> amf_size,
    pub GetPropertyAt: unsafe extern "system" fn(
        pThis: *mut AMFSurface,
        index: amf_size,
        name: *mut u16,
        nameSize: amf_size,
        pValue: *mut AMFVariantStruct,
    ) -> AmfResult,
    pub Clear: unsafe extern "system" fn(pThis: *mut AMFSurface) -> AmfResult,
    pub AddTo: unsafe extern "system" fn(
        pThis: *mut AMFSurface,
        pDest: *mut AMFPropertyStorage,
        overwrite: amf_bool,
        deep: amf_bool,
    ) -> AmfResult,
    pub CopyTo: unsafe extern "system" fn(
        pThis: *mut AMFSurface,
        pDest: *mut AMFPropertyStorage,
        deep: amf_bool,
    ) -> AmfResult,
    pub AddObserver: unsafe extern "system" fn(
        pThis: *mut AMFSurface,
        pObserver: *mut AMFPropertyStorageObserver,
    ),
    pub RemoveObserver: unsafe extern "system" fn(
        pThis: *mut AMFSurface,
        pObserver: *mut AMFPropertyStorageObserver,
    ),

    // AMFData (slots 13..=22)
    pub GetMemoryType: unsafe extern "system" fn(pThis: *mut AMFSurface) -> AMF_MEMORY_TYPE,
    pub Duplicate: unsafe extern "system" fn(
        pThis: *mut AMFSurface,
        memType: AMF_MEMORY_TYPE,
        ppData: *mut *mut AMFData,
    ) -> AmfResult,
    pub Convert:
        unsafe extern "system" fn(pThis: *mut AMFSurface, memType: AMF_MEMORY_TYPE) -> AmfResult,
    pub Interop:
        unsafe extern "system" fn(pThis: *mut AMFSurface, memType: AMF_MEMORY_TYPE) -> AmfResult,
    pub GetDataType: unsafe extern "system" fn(pThis: *mut AMFSurface) -> AMF_DATA_TYPE,
    pub IsReusable: unsafe extern "system" fn(pThis: *mut AMFSurface) -> amf_bool,
    pub SetPts: unsafe extern "system" fn(pThis: *mut AMFSurface, pts: amf_pts),
    pub GetPts: unsafe extern "system" fn(pThis: *mut AMFSurface) -> amf_pts,
    pub SetDuration: unsafe extern "system" fn(pThis: *mut AMFSurface, duration: amf_pts),
    pub GetDuration: unsafe extern "system" fn(pThis: *mut AMFSurface) -> amf_pts,

    // AMFSurface (slots 23..=32)
    pub GetFormat: unsafe extern "system" fn(pThis: *mut AMFSurface) -> AMF_SURFACE_FORMAT,
    pub GetPlanesCount: unsafe extern "system" fn(pThis: *mut AMFSurface) -> amf_size,
    pub GetPlaneAt:
        unsafe extern "system" fn(pThis: *mut AMFSurface, index: amf_size) -> *mut AMFPlane,
    pub GetPlane:
        unsafe extern "system" fn(pThis: *mut AMFSurface, planeType: AMF_PLANE_TYPE) -> *mut AMFPlane,
    pub GetFrameType: unsafe extern "system" fn(pThis: *mut AMFSurface) -> AMF_FRAME_TYPE,
    pub SetFrameType: unsafe extern "system" fn(pThis: *mut AMFSurface, frameType: AMF_FRAME_TYPE),
    pub SetCrop: unsafe extern "system" fn(
        pThis: *mut AMFSurface,
        x: amf_int32,
        y: amf_int32,
        width: amf_int32,
        height: amf_int32,
    ) -> AmfResult,
    pub CopySurfaceRegion: unsafe extern "system" fn(
        pThis: *mut AMFSurface,
        pDest: *mut AMFSurface,
        dstX: amf_int32,
        dstY: amf_int32,
        srcX: amf_int32,
        srcY: amf_int32,
        width: amf_int32,
        height: amf_int32,
    ) -> AmfResult,
    pub AddObserver_Surface:
        unsafe extern "system" fn(pThis: *mut AMFSurface, pObserver: *mut AMFSurfaceObserver),
    pub RemoveObserver_Surface:
        unsafe extern "system" fn(pThis: *mut AMFSurface, pObserver: *mut AMFSurfaceObserver),
}

// --------------------------------------------------------------------------
// DLL entry-point signatures (`extern "C"`)
// --------------------------------------------------------------------------

pub type AMFQueryVersionFn = unsafe extern "C" fn(pVersion: *mut amf_uint64) -> AmfResult;
pub type AMFInitFn =
    unsafe extern "C" fn(version: amf_uint64, ppFactory: *mut *mut AMFFactory) -> AmfResult;

// --------------------------------------------------------------------------
// Dynamic loader for `amfrt64.dll` (Windows only)
// --------------------------------------------------------------------------

#[cfg(windows)]
mod win_loader {
    use std::ffi::c_void;

    /// `LOAD_LIBRARY_SEARCH_SYSTEM32` — prevents DLL planting from cwd / app dir.
    pub const LOAD_LIBRARY_SEARCH_SYSTEM32: u32 = 0x0000_0800;

    #[link(name = "kernel32")]
    extern "system" {
        pub fn LoadLibraryExW(
            lp_lib_file_name: *const u16,
            h_file: *mut c_void,
            dw_flags: u32,
        ) -> *mut c_void;
        pub fn GetProcAddress(h_module: *mut c_void, lp_proc_name: *const u8) -> *const c_void;
        pub fn FreeLibrary(h_module: *mut c_void) -> i32;
    }
}

/// Process-lifetime handle to the AMD AMF runtime (`amfrt64.dll`) and its singleton `AMFFactory`.
///
/// Kept alive in a process-wide [`std::sync::OnceLock`] via [`AmfLibrary::global`] so that
/// `amfrt64.dll` is never unloaded prematurely while AMF components or background threads
/// are tearing down, and so `AMFInit` is invoked exactly once.
#[cfg(windows)]
pub struct AmfLibrary {
    _module: *mut c_void,
    factory: *mut AMFFactory,
    runtime_version: amf_uint64,
}

#[cfg(windows)]
// SAFETY: `amfrt64.dll` module handle and the singleton `AMFFactory` are process-global,
// immutable after `AMFInit`, and thread-safe for `CreateContext` / `CreateComponent`.
unsafe impl Send for AmfLibrary {}
#[cfg(windows)]
unsafe impl Sync for AmfLibrary {}

#[cfg(windows)]
impl AmfLibrary {
    /// Returns the process-wide initialized AMF runtime, loading `amfrt64.dll` on first call.
    pub fn global() -> std::result::Result<&'static Self, String> {
        static RUNTIME: std::sync::OnceLock<std::result::Result<AmfLibrary, String>> =
            std::sync::OnceLock::new();
        match RUNTIME.get_or_init(Self::load_internal) {
            Ok(lib) => Ok(lib),
            Err(e) => Err(e.clone()),
        }
    }

    fn load_internal() -> std::result::Result<Self, String> {
        unsafe {
            let dll = wide_nul("amfrt64.dll");
            let module = win_loader::LoadLibraryExW(
                dll.as_ptr(),
                std::ptr::null_mut(),
                win_loader::LOAD_LIBRARY_SEARCH_SYSTEM32,
            );
            if module.is_null() {
                return Err(
                    "amfrt64.dll not found in System32 (AMD Radeon driver not installed)".into(),
                );
            }

            let qv_sym = win_loader::GetProcAddress(module, c"AMFQueryVersion".as_ptr().cast());
            let init_sym = win_loader::GetProcAddress(module, c"AMFInit".as_ptr().cast());
            if qv_sym.is_null() || init_sym.is_null() {
                win_loader::FreeLibrary(module);
                return Err("amfrt64.dll missing AMFQueryVersion or AMFInit export".into());
            }

            let query_version: AMFQueryVersionFn = std::mem::transmute(qv_sym);
            let amf_init: AMFInitFn = std::mem::transmute(init_sym);

            let mut ver: amf_uint64 = 0;
            let res = query_version(&mut ver);
            if res != AMF_OK {
                win_loader::FreeLibrary(module);
                return Err(format!(
                    "AMFQueryVersion failed: {} ({res})",
                    result_name(res)
                ));
            }
            let major = amf_get_major_version(ver);
            let minor = amf_get_minor_version(ver);
            if major != 1 || minor < 4 {
                win_loader::FreeLibrary(module);
                return Err(format!(
                    "unsupported AMF runtime version {} (need 1.4+)",
                    format_amf_version(ver)
                ));
            }

            let req_ver = ver.min(AMF_MAX_INIT_VERSION);
            let mut factory: *mut AMFFactory = std::ptr::null_mut();
            let res = amf_init(req_ver, &mut factory);
            if res != AMF_OK || factory.is_null() {
                win_loader::FreeLibrary(module);
                return Err(format!(
                    "AMFInit({}) failed: {} ({res})",
                    format_amf_version(req_ver),
                    result_name(res)
                ));
            }

            Ok(Self {
                _module: module,
                factory,
                runtime_version: ver,
            })
        }
    }

    #[inline]
    pub fn factory(&self) -> *mut AMFFactory {
        self.factory
    }

    #[inline]
    pub fn runtime_version(&self) -> amf_uint64 {
        self.runtime_version
    }

    pub fn version_string(&self) -> String {
        format_amf_version(self.runtime_version)
    }
}

// --------------------------------------------------------------------------
// Layout & helper unit tests (run on Linux and Windows)
// --------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::mem::{align_of, offset_of, size_of};

    #[test]
    fn variant_struct_matches_win64_abi() {
        assert_eq!(size_of::<AMFVariantStruct>(), 24);
        assert_eq!(align_of::<AMFVariantStruct>(), 8);
        assert_eq!(offset_of!(AMFVariantStruct, variant_type), 0);
        assert_eq!(offset_of!(AMFVariantStruct, value), 8);
    }

    #[test]
    fn primitive_and_guid_layouts() {
        assert_eq!(size_of::<amf_long>(), 4);
        assert_eq!(size_of::<amf_bool>(), 1);
        assert_eq!(size_of::<amf_pts>(), 8);
        assert_eq!(size_of::<AMFGuid>(), 16);
        assert_eq!(size_of::<AMFSize>(), 8);
        assert_eq!(size_of::<AMFRate>(), 8);
        assert_eq!(size_of::<AMFRect>(), 16);
    }

    #[test]
    fn vtable_slot_counts_match_amf_headers() {
        let ptr = size_of::<*const c_void>();
        assert_eq!(size_of::<AMFInterfaceVtbl>(), 3 * ptr);
        assert_eq!(size_of::<AMFFactoryVtbl>(), 7 * ptr);
        assert_eq!(size_of::<AMFContextVtbl>(), 55 * ptr);
        assert_eq!(size_of::<AMFComponentVtbl>(), 28 * ptr);
        assert_eq!(size_of::<AMFDataVtbl>(), 23 * ptr);
        assert_eq!(size_of::<AMFBufferVtbl>(), 28 * ptr);
        assert_eq!(size_of::<AMFSurfaceVtbl>(), 33 * ptr);
    }

    #[test]
    fn version_packing_roundtrip() {
        let v = amf_make_full_version(1, 4, 18, 0);
        assert_eq!(amf_get_major_version(v), 1);
        assert_eq!(amf_get_minor_version(v), 4);
        assert_eq!(amf_get_subminor_version(v), 18);
        assert_eq!(amf_get_build_version(v), 0);
        assert_eq!(format_amf_version(v), "1.4.18");
        assert!(amf_make_full_version(1, 4, 36, 0) > v);
    }

    #[test]
    fn wide_nul_encodes_utf16_with_terminator() {
        let w = wide_nul("AMF");
        assert_eq!(w, vec![b'A' as u16, b'M' as u16, b'F' as u16, 0]);
        let empty = wide_nul("");
        assert_eq!(empty, vec![0]);
    }

    #[test]
    fn variant_constructors_populate_expected_fields() {
        unsafe {
            let b = AMFVariantStruct::from_bool(true);
            assert_eq!(b.variant_type, AMF_VARIANT_BOOL);
            assert_eq!(b.value.bool_value, 1);

            let i = AMFVariantStruct::from_i64(14_000_000);
            assert_eq!(i.variant_type, AMF_VARIANT_INT64);
            assert_eq!(i.value.int64_value, 14_000_000);

            let sz = AMFVariantStruct::from_size(1920, 1080);
            assert_eq!(sz.variant_type, AMF_VARIANT_SIZE);
            assert_eq!(sz.value.size_value, AMFSize { width: 1920, height: 1080 });

            let r = AMFVariantStruct::from_rate(120, 1);
            assert_eq!(r.variant_type, AMF_VARIANT_RATE);
            assert_eq!(r.value.rate_value, AMFRate { num: 120, den: 1 });
        }
    }
}
