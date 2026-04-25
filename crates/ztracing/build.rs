use std::env;

fn main() {
    let enable_tracing =
        env::var_os("ZTRACING").is_some() || env::var_os("QUORP_TRACING").is_some();
    let enable_memory = env::var_os("ZTRACING_WITH_MEMORY").is_some()
        || env::var_os("QUORP_TRACING_WITH_MEMORY").is_some();

    if enable_tracing {
        println!("cargo::rustc-cfg=quorp_tracing");
    }
    if enable_memory {
        println!("cargo::rustc-cfg=quorp_tracing_with_memory");
    }
    println!("cargo::rerun-if-changed=build.rs");
    println!("cargo::rerun-if-env-changed=ZTRACING");
    println!("cargo::rerun-if-env-changed=QUORP_TRACING");
    println!("cargo::rerun-if-env-changed=ZTRACING_WITH_MEMORY");
    println!("cargo::rerun-if-env-changed=QUORP_TRACING_WITH_MEMORY");
}
