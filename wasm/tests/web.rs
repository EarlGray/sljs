//! Test suite for the Web and headless browsers.

#![cfg(target_arch = "wasm32")]

extern crate wasm_bindgen_test;
use wasm_bindgen_test::*;
//wasm_bindgen_test_configure!(run_in_browser);

use wasm_bindgen::JsValue;

use sljs::{self, ToESTree};

#[wasm_bindgen_test]
fn test_interpret() {
    #[rustfmt::skip]
    use sljs::ast::{ expr, stmt };

    let var_x = sljs::Program::from_stmt(
        stmt::var([("x", expr::lit(12))].iter())
    ).to_estree();

    let x_plus = sljs::Program::from_stmt(
        expr::add(expr::id("x"), expr::lit(8))
    ).to_estree();

    let var_x = JsValue::from_serde(&var_x).unwrap();
    sljs_wasm::interpret(&var_x).unwrap();

    let x_plus = JsValue::from_serde(&x_plus).unwrap();
    assert_eq!(sljs_wasm::interpret(&x_plus), Ok(JsValue::from("20")));
}
