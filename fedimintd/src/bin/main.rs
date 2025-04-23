use fedimint_core::fedimint_build_code_version_env;
#[cfg(not(any(target_env = "msvc", target_os = "ios")))]
use tikv_jemallocator::Jemalloc;

#[cfg(not(any(target_env = "msvc", target_os = "ios")))]
#[global_allocator]
// rocksdb suffers from memory fragmentation when using standard allocator
static GLOBAL: Jemalloc = Jemalloc;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    fedimintd::run(
        fedimintd::default_modules,
        fedimint_build_code_version_env!(),
        None,
    )
    .await
}
