# JNI: giữ lại để Rust gọi ngược (callback) bằng tên.
-keep class com.remoteview.client.NativeBridge { *; }
-keep interface com.remoteview.client.NativeBridge$Callbacks { *; }
