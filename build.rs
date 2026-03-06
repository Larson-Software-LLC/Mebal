fn main() {
    // Additional Windows system libraries required when statically linking FFmpeg.
    // The ffmpeg-sys-next build script handles the basics (ole32, secur32, ws2_32,
    // bcrypt, user32) but misses DirectShow and MediaFoundation dependencies.
    // Safe to include unconditionally — the linker only pulls in what's referenced.
    #[cfg(target_os = "windows")]
    {
        // DirectShow (used by avdevice's gdigrab/dshow capture)
        println!("cargo:rustc-link-lib=strmiids");
        println!("cargo:rustc-link-lib=quartz");
        println!("cargo:rustc-link-lib=oleaut32");
        println!("cargo:rustc-link-lib=uuid");

        // MediaFoundation (used by avcodec's mfenc encoder)
        println!("cargo:rustc-link-lib=mfplat");
        println!("cargo:rustc-link-lib=mf");
        println!("cargo:rustc-link-lib=mfuuid");
    }
}
