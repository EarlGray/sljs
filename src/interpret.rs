use crate::error::TypeError;
use crate::prelude::*;
use crate::Jump;

use crate::ast::*; // yes, EVERYTHING
use crate::builtin;
use crate::{
    function::Closure, object::Access, CallContext, Exception, Heap, Interpreted, JSObject,
    JSResult, JSValue,
};

// ==============================================
/// Describes things (i.e. AST nodes, [`Function`]) that can be interpreted on a [`Heap`]
pub trait Interpretable {
    /// A wrapper for `.interpret` that also resolves the result to JSValue
    fn evaluate(&self, heap: &mut Heap) -> JSResult<JSValue> {
        self.interpret(heap)?.to_value(heap)
    }

    /// Interpret `self` on the `heap`, potentially to a settable [`Interpreted::Member`].
    fn interpret(&self, heap: &mut Heap) -> JSResult<Interpreted>;
}

// ==============================================

impl Interpretable for Program {
    fn interpret(&self, heap: &mut Heap) -> JSResult<Interpreted> {
        heap.declare(self.variables.iter(), self.functions.iter())?;
        self.body.interpret(heap)
    }
}

// ==============================================

impl Interpretable for Statement {
    fn interpret(&self, heap: &mut Heap) -> JSResult<Interpreted> {
        heap.loc = self.loc.clone();
        match &self.stmt {
            Stmt::Empty => Ok(Interpreted::VOID),
            Stmt::Expr(stmt) => stmt.interpret(heap),
            Stmt::Block(stmt) => stmt.interpret(heap),
            Stmt::If(stmt) => stmt.interpret(heap),
            Stmt::Switch(stmt) => stmt.interpret(heap),
            Stmt::For(stmt) => stmt.interpret(heap),
            Stmt::ForIn(stmt) => stmt.interpret(heap),
            Stmt::Break(stmt) => stmt.interpret(heap),
            Stmt::Continue(stmt) => stmt.interpret(heap),
            Stmt::Label(stmt) => stmt.interpret(heap),
            Stmt::Return(stmt) => stmt.interpret(heap),
            Stmt::Throw(stmt) => stmt.interpret(heap),
            Stmt::Try(stmt) => stmt.interpret(heap),
            Stmt::Variable(stmt) => stmt.interpret(heap),
            Stmt::Function(stmt) => stmt.interpret(heap),
        }
    }
}

// ==============================================

impl Interpretable for BlockStatement {
    fn interpret(&self, heap: &mut Heap) -> JSResult<Interpreted> {
        let this_ref = heap.interpret_this();
        let outer_scope = heap.local_scope().unwrap_or(Heap::GLOBAL);
        heap.enter_new_scope(this_ref, outer_scope, |heap| {
            heap.declare(self.bindings.iter(), [].into_iter())?;

            let mut result = Interpreted::VOID;
            for stmt in self.body.iter() {
                result = stmt.interpret(heap)?;
            }
            Ok(result)
        })
    }
}

impl Interpretable for IfStatement {
    fn interpret(&self, heap: &mut Heap) -> JSResult<Interpreted> {
        let cond = self.test.evaluate(heap)?;
        if cond.boolify(heap) {
            self.consequent.interpret(heap)
        } else if let Some(else_stmt) = self.alternate.as_ref() {
            else_stmt.interpret(heap)
        } else {
            Ok(Interpreted::VOID)
        }
    }
}

impl Interpretable for SwitchStatement {
    fn interpret(&self, heap: &mut Heap) -> JSResult<Interpreted> {
        let switchval = self.discriminant.evaluate(heap)?;

        let mut default: Option<usize> = None; // the index of the default case, if any
        let mut found_case: Option<usize> = None;

        // search
        for (i, case) in self.cases.iter().enumerate() {
            let caseval = match &case.test {
                None => {
                    default = Some(i);
                    continue;
                }
                Some(test) => test.evaluate(heap)?,
            };

            if JSValue::strict_eq(&switchval, &caseval, heap) {
                found_case = Some(i);
                break;
            }
        }

        let end = self.cases.len();
        let restart_index = found_case.or(default).unwrap_or(end);

        // execute
        for i in restart_index..end {
            for stmt in self.cases[i].consequent.iter() {
                match stmt.interpret(heap) {
                    Ok(_) => (),
                    Err(Exception::Jump(Jump::Break(None))) => {
                        return Ok(Interpreted::VOID);
                    }
                    Err(e) => return Err(e),
                }
            }
        }
        Ok(Interpreted::VOID)
    }
}

impl ForStatement {
    /// `do_loop()` executes the loop except its `init` statement.
    /// `init` must be interpreted before this, if needed.
    fn do_loop(&self, heap: &mut Heap) -> Result<(), Exception> {
        while self.should_iterate(heap)? {
            // body
            let result = self.body.interpret(heap);
            match result {
                Ok(_) => (),
                Err(Exception::Jump(Jump::Continue(None))) => (),
                Err(Exception::Jump(Jump::Break(None))) => break,
                Err(e) => return Err(e),
            };

            self.do_update(heap)?;
        }
        Ok(())
    }

    fn should_iterate(&self, heap: &mut Heap) -> JSResult<bool> {
        match self.test.as_ref() {
            None => Ok(true),
            Some(testexpr) => {
                let result = testexpr.evaluate(heap)?;
                Ok(result.boolify(heap))
            }
        }
    }

    fn do_update(&self, heap: &mut Heap) -> Result<(), Exception> {
        if let Some(updateexpr) = self.update.as_ref() {
            updateexpr.interpret(heap)?;
        }
        Ok(())
    }
}

impl Interpretable for ForStatement {
    fn interpret(&self, heap: &mut Heap) -> JSResult<Interpreted> {
        self.init.interpret(heap)?;
        self.do_loop(heap)?;
        Ok(Interpreted::VOID)
    }
}

impl ForInStatement {}

impl Interpretable for ForInStatement {
    fn interpret(&self, heap: &mut Heap) -> JSResult<Interpreted> {
        let iteratee = self.right.evaluate(heap)?.objectify(heap);

        let assignexpr = match &self.left {
            ForInTarget::Expr(expr) => expr.clone(),
            ForInTarget::Var(vardecl) => {
                debug_assert_eq!(vardecl.declarations.len(), 1);
                let ident = &vardecl.declarations[0].name;
                let idexpr = Expr::Identifier(Identifier::from(ident.as_str()));
                Expression {
                    expr: idexpr,
                    loc: None,
                }
            }
        };

        let mut visited = HashSet::new();
        let mut objref = iteratee;
        while objref != Heap::NULL {
            let object = heap.get(objref);
            let mut keys = (object.properties.keys())
                .cloned()
                .collect::<HashSet<JSString>>();
            if let Some(array) = object.as_array() {
                let indices = 0..array.storage.len();
                keys.extend(indices.map(|i| i.to_string().into()));
            }
            // TODO: strings iteration

            for propname in keys.drain() {
                if visited.contains(&propname) {
                    continue;
                }
                visited.insert(propname.clone());

                let object = heap.get(objref);
                match object.properties.get(&propname) {
                    Some(p) if p.access.enumerable() => (),
                    None if object.as_array().is_some() && propname.parse::<usize>().is_ok() => (),
                    Some(_) => continue, // not enumerable, skip
                    None => continue,    // the property has disappeared!
                };

                let propname = match propname.parse::<usize>() {
                    Ok(p) => JSValue::from(p as f64),
                    _ => JSValue::from(propname.as_str()),
                };
                assignexpr
                    .interpret(heap)?
                    .put_value(propname, heap)
                    .or_else(crate::error::ignore_set_readonly)?;

                match self.body.interpret(heap) {
                    Ok(_) => (),
                    Err(Exception::Jump(Jump::Continue(None))) => continue,
                    Err(Exception::Jump(Jump::Break(None))) => {
                        return Ok(Interpreted::VOID);
                    }
                    Err(e) => {
                        return Err(e);
                    }
                }
            }

            objref = heap.get(objref).proto;
        }
        Ok(Interpreted::VOID)
    }
}

impl Interpretable for BreakStatement {
    fn interpret(&self, _heap: &mut Heap) -> JSResult<Interpreted> {
        let BreakStatement(maybe_label) = self;
        Err(Exception::Jump(Jump::Break(maybe_label.clone())))
    }
}

impl Interpretable for ContinueStatement {
    fn interpret(&self, _heap: &mut Heap) -> JSResult<Interpreted> {
        let ContinueStatement(maybe_label) = self;
        Err(Exception::Jump(Jump::Continue(maybe_label.clone())))
    }
}

impl LabelStatement {
    fn continue_loop(&self, heap: &mut Heap) -> JSResult<Interpreted> {
        let LabelStatement(label, body) = self;
        loop {
            // must be a loop to continue
            let loop_stmt = match &body.stmt {
                Stmt::For(stmt) => stmt,
                Stmt::ForIn(_) => todo!(),
                // TODO: move this check into the parser?
                _ => return Err(Exception::no_loop_for_continue_label(label.clone())),
            };

            loop_stmt.do_update(heap)?;
            let result = loop_stmt.do_loop(heap);
            match result {
                Err(Exception::Jump(Jump::Continue(Some(target)))) if &target == label => continue,
                Err(Exception::Jump(Jump::Break(Some(target)))) if &target == label => break,
                Err(e) => return Err(e),
                Ok(()) => break,
            }
        }
        Ok(Interpreted::VOID)
    }
}

impl Interpretable for LabelStatement {
    fn interpret(&self, heap: &mut Heap) -> JSResult<Interpreted> {
        let LabelStatement(label, body) = self;

        let result = body.interpret(heap);
        match result {
            Err(Exception::Jump(Jump::Break(Some(target)))) if &target == label => {
                Ok(Interpreted::VOID)
            }
            Err(Exception::Jump(Jump::Continue(Some(target)))) if &target == label => {
                self.continue_loop(heap)
            }
            _ => result,
        }
    }
}

impl Interpretable for ExpressionStatement {
    fn interpret(&self, heap: &mut Heap) -> JSResult<Interpreted> {
        let value = self.expression.evaluate(heap)?;
        Ok(Interpreted::Value(value))
    }
}

impl Interpretable for ReturnStatement {
    fn interpret(&self, heap: &mut Heap) -> JSResult<Interpreted> {
        let ReturnStatement(argument) = self;
        let returned = match argument {
            None => Interpreted::VOID,
            Some(argexpr) => argexpr.interpret(heap)?,
        };
        Err(Exception::Jump(Jump::Return(returned)))
    }
}

impl Interpretable for ThrowStatement {
    fn interpret(&self, heap: &mut Heap) -> JSResult<Interpreted> {
        let ThrowStatement(exc_expr) = self;
        let exc_value = exc_expr.evaluate(heap)?;
        heap.throw(Exception::UserThrown(exc_value))
    }
}

impl CatchClause {
    fn interpret(&self, exc: &Exception, heap: &mut Heap) -> JSResult<Interpreted> {
        let this_ref = heap.interpret_this();
        let scope_ref = heap.local_scope().unwrap_or(Heap::GLOBAL);

        heap.enter_new_scope(this_ref, scope_ref, |heap| {
            let error_value: JSValue = match exc {
                Exception::UserThrown(errval) => errval.clone(),
                Exception::Jump(_) => {
                    panic!("Impossible to catch: {:?}", exc)
                }
                //Exception::ReferenceNotFound(ident) => { // TODO: ReferenceError
                _ => {
                    let message = format!("{:?}", exc);
                    let args = vec![Interpreted::from(message)];
                    let errval = builtin::error::error_constructor(
                        CallContext::from(args)
                            .with_this(this_ref)
                            .with_name("Error".into()),
                        heap,
                    )?;
                    errval.to_value(heap)?
                }
            };

            heap.scope_mut()
                .set_nonconf(self.param.0.as_str(), error_value)?;
            self.body.interpret(heap)
        })
    }
}

impl TryStatement {
    fn run_finalizer(&self, heap: &mut Heap) -> Result<(), Exception> {
        if let Some(finalizer) = self.finalizer.as_ref() {
            finalizer.interpret(heap)?;
        }
        Ok(())
    }
}

impl Interpretable for TryStatement {
    fn interpret(&self, heap: &mut Heap) -> JSResult<Interpreted> {
        let result = self.block.interpret(heap);
        match &result {
            Ok(_) | Err(Exception::Jump(_)) => {
                self.run_finalizer(heap)?;
                result
            }
            Err(exc) => {
                let result = match &self.handler {
                    None => result,
                    Some(catch) => catch.interpret(exc, heap),
                };
                self.run_finalizer(heap)?;
                result
            }
        }
    }
}

impl Interpretable for VariableDeclaration {
    fn interpret(&self, heap: &mut Heap) -> JSResult<Interpreted> {
        for decl in &self.declarations {
            if let Some(initexpr) = decl.init.as_ref() {
                let name = &decl.name.0;
                let value = initexpr.evaluate(heap)?;
                match heap.lookup_var(name) {
                    Some(Interpreted::Member { of, name }) => {
                        heap.get_mut(of)
                            .set_property(name.as_str(), value)
                            .or_else(crate::error::ignore_set_readonly)?;
                    }
                    _ => panic!("variable not declared: {}", name),
                }
            }
        }
        Ok(Interpreted::VOID)
    }
}

impl Interpretable for FunctionDeclaration {
    fn interpret(&self, _heap: &mut Heap) -> JSResult<Interpreted> {
        // no-op: the work in done in Closure::call()
        Ok(Interpreted::VOID)
    }
}

impl Interpretable for Expression {
    fn interpret(&self, heap: &mut Heap) -> JSResult<Interpreted> {
        heap.loc = self.loc.clone();
        match &self.expr {
            Expr::Literal(expr) => expr.interpret(heap),
            Expr::Identifier(expr) => expr.interpret(heap),
            Expr::BinaryOp(expr) => expr.interpret(heap),
            Expr::LogicalOp(expr) => expr.interpret(heap),
            Expr::Call(expr) => expr.interpret(heap),
            Expr::Array(expr) => expr.interpret(heap),
            Expr::Member(expr) => expr.interpret(heap),
            Expr::Object(expr) => expr.interpret(heap),
            Expr::Assign(expr) => expr.interpret(heap),
            Expr::Conditional(expr) => expr.interpret(heap),
            Expr::Unary(expr) => expr.interpret(heap),
            Expr::Update(expr) => expr.interpret(heap),
            Expr::Sequence(expr) => expr.interpret(heap),
            Expr::Function(expr) => expr.interpret(heap),
            Expr::New(expr) => expr.interpret(heap),
            Expr::This => Ok(Interpreted::from(heap.interpret_this())),
        }
    }
}

impl Interpretable for Literal {
    fn interpret(&self, _heap: &mut Heap) -> JSResult<Interpreted> {
        let value = self.to_value();
        Ok(Interpreted::Value(value))
    }
}

impl Interpretable for Identifier {
    fn interpret(&self, heap: &mut Heap) -> JSResult<Interpreted> {
        let name = &self.0;
        let place = heap
            .lookup_var(name)
            .unwrap_or_else(|| Interpreted::member(Heap::GLOBAL, name));
        Ok(place)
    }
}

impl Interpretable for ConditionalExpression {
    fn interpret(&self, heap: &mut Heap) -> JSResult<Interpreted> {
        let cond = self.condexpr.evaluate(heap)?;
        if cond.boolify(heap) {
            self.thenexpr.interpret(heap)
        } else {
            self.elseexpr.interpret(heap)
        }
    }
}

impl Interpretable for LogicalExpression {
    fn interpret(&self, heap: &mut Heap) -> JSResult<Interpreted> {
        let LogicalExpression(lexpr, op, rexpr) = self;
        let lval = lexpr.evaluate(heap)?;
        let value = match (lval.boolify(heap), op) {
            (true, BoolOp::And) | (false, BoolOp::Or) => rexpr.evaluate(heap)?,
            _ => lval,
        };
        Ok(Interpreted::Value(value))
    }
}

impl BinOp {
    fn compute(&self, lval: &JSValue, rval: &JSValue, heap: &mut Heap) -> JSResult<JSValue> {
        Ok(match self {
            BinOp::EqEq => JSValue::from(JSValue::loose_eq(lval, rval, heap)),
            BinOp::NotEq => JSValue::from(!JSValue::loose_eq(lval, rval, heap)),
            BinOp::EqEqEq => JSValue::from(JSValue::strict_eq(lval, rval, heap)),
            BinOp::NotEqEq => JSValue::from(!JSValue::strict_eq(lval, rval, heap)),
            BinOp::Less => JSValue::compare(lval, rval, heap, |a, b| a < b, |a, b| a < b),
            BinOp::Greater => JSValue::compare(lval, rval, heap, |a, b| a > b, |a, b| a > b),
            BinOp::LtEq => JSValue::compare(lval, rval, heap, |a, b| a <= b, |a, b| a <= b),
            BinOp::GtEq => JSValue::compare(lval, rval, heap, |a, b| a >= b, |a, b| a >= b),
            BinOp::Plus => JSValue::plus(lval, rval, heap)?,
            BinOp::Minus => JSValue::minus(lval, rval, heap)?,
            BinOp::Star => JSValue::numerically(lval, rval, heap, |a, b| a * b),
            BinOp::Slash => JSValue::numerically(lval, rval, heap, |a, b| a / b),
            BinOp::Percent => JSValue::numerically(lval, rval, heap, |a, b| a % b),
            BinOp::Pipe => {
                let bitor = |a, b| (a as i32 | b as i32) as f64;
                JSValue::numerically(lval, rval, heap, bitor)
            }
            BinOp::Hat => {
                let bitxor = |a, b| (a as i32 ^ b as i32) as f64;
                JSValue::numerically(lval, rval, heap, bitxor)
            }
            BinOp::Ampersand => {
                let bitand = |a, b| (a as i32 & b as i32) as f64;
                JSValue::numerically(lval, rval, heap, bitand)
            }
            BinOp::LtLt => {
                let bitshl = |a, b| ((a as i32) << ((b as u32) & 0x1f) as i32) as f64;
                JSValue::numerically(lval, rval, heap, bitshl)
            }
            BinOp::GtGt => {
                let bitshr = |a, b| ((a as i32) >> ((b as u32) & 0x1f) as i32) as f64;
                JSValue::numerically(lval, rval, heap, bitshr)
            }
            BinOp::GtGtGt => {
                let bitshru = |a, b| ((a as u32) >> (b as u32) & 0x1f) as f64;
                JSValue::numerically(lval, rval, heap, bitshru)
            }
            BinOp::In => {
                let prop = lval.stringify(heap)?;
                let objref = rval.to_ref()?;
                let object = heap.get(objref);
                let found = object.lookup_value(&prop, heap).is_some();
                JSValue::from(found)
            }
            BinOp::InstanceOf => {
                let constructor = rval.to_ref()?;
                let found = match lval.to_ref() {
                    Err(_) => false,
                    Ok(objref) => objref.isinstance(constructor, heap)?,
                };
                JSValue::from(found)
            }
        })
    }
}

impl Interpretable for BinaryExpression {
    fn interpret(&self, heap: &mut Heap) -> JSResult<Interpreted> {
        let BinaryExpression(lexpr, op, rexpr) = self;
        let lval = lexpr.evaluate(heap)?;
        let rval = rexpr.evaluate(heap)?;
        let result = op.compute(&lval, &rval, heap)?;
        Ok(Interpreted::Value(result))
    }
}

impl Interpretable for UnaryExpression {
    fn interpret(&self, heap: &mut Heap) -> JSResult<Interpreted> {
        let UnaryExpression(op, argexpr) = self;
        let arg = argexpr.interpret(heap)?;
        let argvalue = || arg.to_value(heap);
        let argnum = || argvalue().map(|val| val.numberify(heap).unwrap_or(f64::NAN));
        let value = match op {
            UnOp::Exclamation => JSValue::Bool(!argvalue()?.boolify(heap)),
            UnOp::Minus => JSValue::Number(-argnum()?),
            UnOp::Plus => JSValue::Number(argnum()?),
            UnOp::Tilde => {
                let num = argnum()?;
                let num = if f64::is_nan(num) { 0.0 } else { num };
                JSValue::from(-(1.0 + num))
            }
            UnOp::Void => JSValue::Undefined,
            UnOp::Typeof => JSValue::from(
                argvalue()
                    .map(|val| val.type_of(heap))
                    .unwrap_or("undefined"),
            ),
            UnOp::Delete => JSValue::from(arg.delete(heap).is_ok()),
        };
        Ok(Interpreted::Value(value))
    }
}

impl Interpretable for UpdateExpression {
    fn interpret(&self, heap: &mut Heap) -> JSResult<Interpreted> {
        let UpdateExpression(op, prefix, argexpr) = self;
        let assignee = argexpr.interpret(heap)?;

        let oldvalue = assignee.to_value(heap)?;
        let oldnum = oldvalue.numberify(heap).unwrap_or(f64::NAN);
        let newnum = match op {
            UpdOp::Increment => oldnum + 1.0,
            UpdOp::Decrement => oldnum - 1.0,
        };

        assignee
            .put_value(JSValue::from(newnum), heap)
            .or_else(crate::error::ignore_set_readonly)?;

        let resnum = if *prefix { newnum } else { oldnum };
        Ok(Interpreted::from(resnum))
    }
}

impl Interpretable for SequenceExpression {
    fn interpret(&self, heap: &mut Heap) -> JSResult<Interpreted> {
        let SequenceExpression(exprs) = self;

        let mut value = JSValue::Undefined;
        for expr in exprs.iter() {
            value = expr.interpret(heap)?.to_value(heap)?;
        }
        Ok(Interpreted::from(value))
    }
}

impl Interpretable for MemberExpression {
    fn interpret(&self, heap: &mut Heap) -> JSResult<Interpreted> {
        let MemberExpression(objexpr, propexpr, computed) = self;

        // compute the name of the property:
        let propname = if *computed {
            let propval = propexpr.interpret(heap)?.to_value(heap)?;
            propval.stringify(heap)?
        } else {
            match &propexpr.expr {
                Expr::Identifier(name) => name.0.clone(),
                _ => panic!("Member(computed=false) property is not an identifier"),
            }
        };

        // get the object reference for member computation:
        let objresult = objexpr.interpret(heap)?;
        let objref = match objresult.to_value(heap)? {
            JSValue::Undefined => return Err(Exception::not_an_object(objresult)),
            value => value.objectify(heap),
        };

        // TODO: __proto__ as (getPrototypeOf, setPrototypeOf) property
        if propname.as_str() == "__proto__" {
            let proto = heap.get(objref).proto;
            return Ok(Interpreted::from(proto));
        }

        Ok(Interpreted::Member {
            of: objref,
            name: propname,
        })
    }
}

impl Interpretable for ObjectExpression {
    fn interpret(&self, heap: &mut Heap) -> JSResult<Interpreted> {
        let mut object = JSObject::new();

        for (key, valexpr) in self.0.iter() {
            let keyname = match key {
                ObjectKey::Identifier(ident) => ident.clone(),
                ObjectKey::Computed(expr) => {
                    let result = expr.interpret(heap)?.to_value(heap)?;
                    result.stringify(heap)?
                }
            };
            let valresult = valexpr.interpret(heap)?;
            let value = valresult.to_value(heap)?;
            object.set_property(keyname.as_str(), value)?;
        }

        let object_ref = heap.alloc(object);
        Ok(Interpreted::from(object_ref))
    }
}

impl Interpretable for ArrayExpression {
    fn interpret(&self, heap: &mut Heap) -> JSResult<Interpreted> {
        let ArrayExpression(exprs) = self;
        let storage = (exprs.iter())
            .map(|expr| expr.interpret(heap)?.to_value(heap))
            .collect::<Result<Vec<JSValue>, Exception>>()?;

        let object = JSObject::from_array(storage);
        let object_ref = heap.alloc(object);
        Ok(Interpreted::from(object_ref))
    }
}

impl Interpretable for AssignmentExpression {
    fn interpret(&self, heap: &mut Heap) -> JSResult<Interpreted> {
        let AssignmentExpression(leftexpr, modop, valexpr) = self;

        let value = valexpr.evaluate(heap)?;

        // This can be:
        // - Interpreted::Member{ existing object, attribute }
        // - Interpreted::Member{ scope, existing variable }
        // - Interpreted::Member{ global, non-existing variable }
        // - Interpreted::Value
        let assignee = leftexpr.interpret(heap)?;

        let newvalue = match modop {
            None => value,
            Some(op) => {
                let oldvalue = assignee.to_value(heap)?;
                op.compute(&oldvalue, &value, heap)?
            }
        };
        assignee
            .put_value(newvalue.clone(), heap)
            .or_else(crate::error::ignore_set_readonly)?;
        Ok(Interpreted::Value(newvalue))
    }
}

impl Interpretable for CallExpression {
    fn interpret(&self, heap: &mut Heap) -> JSResult<Interpreted> {
        let CallExpression(callee_expr, argument_exprs) = self;

        let arguments = (argument_exprs.iter())
            .map(|argexpr| argexpr.interpret(heap))
            .collect::<Result<Vec<Interpreted>, Exception>>()?;

        let callee = callee_expr.interpret(heap)?;
        let (func_ref, this_ref, name) = callee.resolve_call(heap)?;

        heap.execute(
            func_ref,
            CallContext::from(arguments)
                .with_this(this_ref)
                .with_name(name),
        )
    }
}

impl Interpretable for NewExpression {
    fn interpret(&self, heap: &mut Heap) -> JSResult<Interpreted> {
        let NewExpression(callee_expr, argument_exprs) = self;

        let arguments = (argument_exprs.iter())
            .map(|expr| expr.interpret(heap))
            .collect::<Result<Vec<Interpreted>, Exception>>()?;

        let callee = callee_expr.interpret(heap)?;
        let funcref = callee.to_ref(heap)?;
        let prototype_ref = (heap.get_mut(funcref))
            .get_own_value("prototype")
            .ok_or_else(|| {
                Exception::attr_type_error(TypeError::CANNOT_GET_PROPERTY, callee, "prototype")
            })?
            .to_ref()?;

        // allocate the object
        let mut object = JSObject::new();
        object.proto = prototype_ref;

        let object_ref = heap.alloc(object);

        // call its constructor
        let result = heap.execute(
            funcref,
            CallContext::from(arguments)
                .with_this(object_ref)
                .with_name("<constructor>".into()),
        )?;
        match result {
            Interpreted::Value(JSValue::Ref(r)) if r != Heap::NULL => Ok(result),
            _ => Ok(Interpreted::from(object_ref)),
        }
    }
}

impl Interpretable for FunctionExpression {
    fn interpret(&self, heap: &mut Heap) -> JSResult<Interpreted> {
        let closure = Closure {
            function: Rc::clone(&self.func),
            captured_scope: heap.local_scope().unwrap_or(Heap::GLOBAL),
        };

        let function_object = JSObject::from_closure(closure);
        let function_ref = heap.alloc(function_object);

        let prototype_ref = heap.alloc(JSObject::new());
        heap.get_mut(function_ref)
            .define_own_property("prototype", Access::WRITE)?;
        heap.get_mut(function_ref)
            .set_property("prototype", prototype_ref)?;
        heap.get_mut(prototype_ref)
            .set_hidden("constructor", function_ref)?;

        Ok(Interpreted::from(function_ref))
    }
}
