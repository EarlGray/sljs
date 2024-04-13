use core::cell::RefCell;
use core::fmt;
use sljs::{
    Heap,
    Program,
    //Interpretable,
    JSON,
};

use wasm_bindgen::prelude::*;

#[cfg(feature = "wee_alloc")]
#[global_allocator]
static ALLOC: wee_alloc::WeeAlloc = wee_alloc::WeeAlloc::INIT;

thread_local! {
    static HEAP: RefCell<Heap> = RefCell::new(Heap::new());
}

#[wasm_bindgen]
extern "C" {
    fn alert(s: &str);
}

#[wasm_bindgen(start)]
pub fn run() {
    #[cfg(feature = "console_error_panic_hook")]
    console_error_panic_hook::set_once();
}

fn jserror<E: fmt::Debug>(e: E) -> JsValue {
    JsValue::from(format!("{:?}", e))
}

/// Takes a ESTree AST representation and produces a result as a pretty-printed string
#[wasm_bindgen]
pub fn interpret(jsobject: &JsValue) -> Result<JsValue, JsValue> {
    let json: JSON = jsobject.into_serde().map_err(jserror)?;
    let program = Program::parse_from(&json).map_err(jserror)?;
    let result = HEAP
        .with(|heapcell| {
            let mut heap = heapcell.borrow_mut();
            heap.evaluate(&program)?.to_string(&mut heap)
        })
        .map_err(jserror)?;
    JsValue::from_serde(result.as_str()).map_err(jserror)
}
