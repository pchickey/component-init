use anyhow::{Context, Result, anyhow};
use std::sync::OnceLock;
use test_programs_artifacts::{RAW, USING_MACRO};
use wasmtime::{Engine, component::Component};

fn engine() -> &'static Engine {
    static ENGINE: OnceLock<Engine> = OnceLock::new();
    ENGINE.get_or_init(|| {
        let mut config = wasmtime::Config::new();
        config.async_support(true);
        Engine::new(&config).expect("create engine")
    })
}

fn load_component(binary: &[u8]) -> Result<Component> {
    let engine = engine();
    Component::new(engine, binary).context("load component")
}

async fn execute(component: &[u8]) -> Result<()> {
    use wasmtime::{
        Store,
        component::{Linker, ResourceTable},
    };
    use wasmtime_wasi::p2::WasiCtx;
    let engine = engine();
    let component = load_component(component)?;

    struct Ctx {
        table: ResourceTable,
        wasi: WasiCtx,
    }
    impl wasmtime_wasi::p2::IoView for Ctx {
        fn table(&mut self) -> &mut ResourceTable {
            &mut self.table
        }
    }
    impl wasmtime_wasi::p2::WasiView for Ctx {
        fn ctx(&mut self) -> &mut WasiCtx {
            &mut self.wasi
        }
    }

    let mut linker = Linker::new(engine);
    wasmtime_wasi::p2::add_to_linker_async(&mut linker).context("add wasi to linker")?;
    let mut store = Store::new(
        engine,
        Ctx {
            table: ResourceTable::new(),
            wasi: WasiCtx::builder().inherit_stdout().inherit_stderr().build(),
        },
    );

    let instance = linker
        .instantiate_async(&mut store, &component)
        .await
        .context("instantiate")?;

    let wasi_cli_run = instance
        .get_export_index(&mut store, None, "wasi:cli/run@0.2.3")
        .ok_or_else(|| anyhow!("`wasi:cli/run` is not exported"))?;
    let export = instance
        .get_export_index(&mut store, Some(&wasi_cli_run), "run")
        .ok_or_else(|| anyhow!("`wasi:cli/run.run` is not exported"))?;
    let func = instance
        .get_func(&mut store, export)
        .ok_or_else(|| anyhow!("`wasi:cli/run.run` export is not a func"))?;
    let func = func
        .typed::<(), (Result<(), ()>,)>(&mut store)
        .context("type of run func")?;
    func.call_async(&mut store, ())
        .await
        .context("executing run")?
        .0
        .map_err(|()| anyhow!("run returned error"))?;
    func.post_return_async(&mut store)
        .await
        .context("post-return run")?;
    Ok(())
}

async fn inits_properly(uninit_component: &[u8]) -> Result<()> {
    // Before initialization, has a component-export func
    let component = load_component(uninit_component)?;
    let (export, _ix) = component
        .get_export(None, "component-init")
        .ok_or_else(|| anyhow!("component should have a component-init export func"))?;
    assert!(
        matches!(
            export,
            wasmtime::component::types::ComponentItem::ComponentFunc(_)
        ),
        "component-init is a func"
    );

    // Without initialization, run will trap.
    let err = execute(uninit_component)
        .await
        .err()
        .context("uninitialized run should trap")?;
    assert!(
        format!("{err:?}").contains("unreachable"),
        "should die with an unreachable trap, got: {err:?}"
    );

    let initialized_component = component_init_wasmtime::initialize(uninit_component).await?;

    // After initialization, will not trap.
    execute(&initialized_component)
        .await
        .context("execute initialized component")?;

    let component = load_component(uninit_component)?;
    assert!(
        component.get_export(None, "component-init").is_none(),
        "component-init func removed from initialized component"
    );

    Ok(())
}

#[tokio::test]
async fn raw() -> Result<()> {
    println!("test component: {RAW:?}");
    let component = std::fs::read(RAW)?;
    inits_properly(&component).await
}

#[tokio::test]
async fn using_macro() -> Result<()> {
    println!("test component: {USING_MACRO:?}");
    let component = std::fs::read(USING_MACRO)?;
    inits_properly(&component).await
}
