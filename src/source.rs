use serde::{
    Deserialize,
    Serialize,
};

use crate::error::ParseError;
use crate::object::Content;
use crate::prelude::*;
use crate::{
    Exception,
    Heap,
    Interpreted,
    JSObject,
    JSValue,
    JSON,
};

const CALLER_LOCATION: &str = "[[caller_location]]";

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
pub struct Position {
    line: usize,
    column: usize,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
pub struct Location {
    start: Position,
    end: Position,
}

impl Location {
    fn from_saved(object: &JSObject, heap: &Heap) -> Result<Location, Exception> {
        if let Some(array) = object.as_array() {
            let line = array.storage[0].numberify(heap).unwrap() as usize;
            let column = array.storage[1].numberify(heap).unwrap() as usize;
            let start = Position { line, column };

            let line = array.storage[2].numberify(heap).unwrap() as usize;
            let column = array.storage[3].numberify(heap).unwrap() as usize;
            let end = Position { line, column };

            Ok(Location { start, end })
        } else {
            Err(Exception::TypeErrorNotArraylike(Interpreted::VOID))
        }
    }
}

#[derive(Clone, Debug)]
pub struct Document {
    _name: String,
}

pub fn save_caller(caller: Option<Box<Location>>, heap: &mut Heap) -> Result<(), Exception> {
    if let Some(loc) = caller {
        let array = vec![
            JSValue::from(loc.start.line as f64),
            JSValue::from(loc.start.column as f64),
            JSValue::from(loc.end.line as f64),
            JSValue::from(loc.end.column as f64),
        ];
        let loc_ref = heap.alloc(JSObject::from_array(array));
        heap.scope_mut()
            .set_system(CALLER_LOCATION, Content::from(loc_ref))?;
    }
    Ok(())
}

// TODO: `struct Callstack` and `impl Display for Callstack`.
struct Callstack<'heap> {
    heap: &'heap Heap,
}

impl<'heap> fmt::Display for Callstack<'heap> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let loc = self.heap.loc.clone();
        writeln!(f, "{:?}", loc)?;

        let mut scoperef = self.heap.local_scope().unwrap_or(Heap::NULL);
        while scoperef != Heap::NULL {
            let loc_ref = (self.heap.get(scoperef))
                .get_value(CALLER_LOCATION)
                .and_then(|v| v.to_ref().ok())
                .ok_or_else(|| {
                    Exception::SyntaxTreeError(ParseError::no_attr(CALLER_LOCATION, JSON::Null))
                })
                .map_err(|_| fmt::Error)?;
            if let Ok(loc) = Location::from_saved(self.heap.get(loc_ref), self.heap) {
                writeln!(f, "   {:?}", loc)?;
            } else {
                writeln!(f, "   ???")?;
            }

            scoperef = match self.heap.get(scoperef).get_value(Heap::SAVED_SCOPE) {
                Some(v) => v.to_ref().unwrap_or(Heap::NULL),
                None => Heap::NULL,
            };
        }

        Ok(())
    }
}

#[cfg(feature = "std")]
pub fn print_callstack(heap: &Heap) -> Result<(), Exception> {
    use std::io::Write;

    let callstack = Callstack { heap };
    let mut stderr = std::io::stderr();
    write!(&mut stderr, "{}", callstack).map_err(|e| {
        let msg = format!("{}", e);
        Exception::UserThrown(JSValue::from(msg))
    })
}

#[cfg(not(feature = "std"))]
pub fn print_callstack(_heap: &Heap) -> Result<(), Exception> {
    unimplemented!()
}
