//! OpenGL ES shims.
//!
//! On device these forward to the host's real GLES driver (Android ships
//! EGL/GLES2/3), which is the whole point of choosing GLES as the graphics
//! surface: no translation layer is required. The registry entries here are the
//! seam where each `gl*` symbol is bound to a thin marshaller that reads
//! arguments from guest memory and calls the native driver. Until wired, they
//! are stubbed to zero so a renderer initialises without faulting.

use super::Registry;

pub fn install(map: &mut Registry) {
    for sym in [
        "glClear",
        "glClearColor",
        "glViewport",
        "glGenBuffers",
        "glBindBuffer",
        "glBufferData",
        "glCreateShader",
        "glCompileShader",
        "glCreateProgram",
        "glUseProgram",
        "glDrawArrays",
        "glDrawElements",
        "glGetError",
        "glFlush",
        "glFinish",
    ] {
        super::register_stub(map, sym);
    }
}
