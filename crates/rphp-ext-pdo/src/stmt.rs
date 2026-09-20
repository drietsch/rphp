//! `PDOStatement`: parameters, execution, the fetch modes and their flags,
//! bound columns, column metadata, iteration.

use rphp_runtime::{nm, Ctx, NativeResult, Registry, Unwind, Visibility};
use rphp_value::{Array, ArrayKey, Object, Payload, PhpRef, Value};

use crate::driver::{Column, DbError, ParamKey, SqlValue};
use crate::pdo::{
    self, clear_error, exception, int_arg, method_ref, param_value, report, str_arg, this, with_conn, with_pdo,
    ErrorState, FetchSpec, ATTR_STRINGIFY_FETCHES, CASE_LOWER, CASE_UPPER, FETCH_ASSOC, FETCH_BOTH, FETCH_BOUND,
    FETCH_CLASS, FETCH_CLASSTYPE, FETCH_COLUMN, FETCH_DEFAULT, FETCH_FUNC, FETCH_GROUP, FETCH_INTO,
    FETCH_KEY_PAIR, FETCH_LAZY, FETCH_MODE_MASK, FETCH_NAMED, FETCH_NUM, FETCH_OBJ, FETCH_PROPS_LATE,
    FETCH_SERIALIZE, FETCH_UNIQUE, NULL_EMPTY_STRING, NULL_TO_STRING, PARAM_INT, PARAM_NULL, PARAM_STR,
};

/// One bound parameter.
#[derive(Clone, Debug)]
pub struct Param {
    pub key: ParamKey,
    /// The value (`bindValue`) or the variable's cell (`bindParam`).
    pub value: Value,
    pub ty: i64,
}

/// One bound column (`bindColumn`).
#[derive(Clone, Debug)]
pub struct BoundColumn {
    /// A 1-based position or a name.
    pub key: ParamKey,
    pub cell: PhpRef,
    pub ty: i64,
}

/// A `PDOStatement` instance's payload.
pub struct StmtState {
    /// The connection.
    pub pdo: Object,
    pub sql: String,
    pub params: Vec<Param>,
    pub columns: Vec<Column>,
    pub rows: Vec<Vec<SqlValue>>,
    pub cursor: usize,
    pub executed: bool,
    pub fetch: Option<FetchSpec>,
    pub bound: Vec<BoundColumn>,
    pub error: ErrorState,
    pub row_count: i64,
}

pub fn with_stmt<R>(o: &Object, f: impl FnOnce(&mut StmtState) -> R) -> Result<R, Unwind> {
    o.with_payload::<StmtState, _>(f)
        .ok_or_else(|| Unwind::error("PDOStatement object is uninitialized"))
}

/// Compile `sql` on the connection and build the statement object
/// (`ATTR_STATEMENT_CLASS` decides which class, and runs its constructor).
pub fn new_statement(ctx: &mut Ctx, pdo: &Object, sql: &str) -> Result<Result<Object, DbError>, Unwind> {
    // Compiling the SQL now is what surfaces a syntax error from
    // `prepare()` rather than `execute()`, as php's driver does.
    if let Err(e) = with_conn(ctx, pdo, |st| st.conn.prepare(sql))? {
        return Ok(Err(e));
    }
    let (class, ctor_args) = with_pdo(pdo, |st| st.attrs.statement_class.clone())?
        .map(|(n, a)| (n.to_vec(), a))
        .unwrap_or_else(|| (b"PDOStatement".to_vec(), Vec::new()));
    let cid = ctx.lookup_class_or_error(&class)?;
    let obj = ctx.instantiate(cid);
    obj.set(b"queryString", Value::string(sql.as_bytes()));
    obj.set_payload(Payload::Native(Box::new(StmtState {
        pdo: pdo.clone(),
        sql: sql.to_string(),
        params: Vec::new(),
        columns: Vec::new(),
        rows: Vec::new(),
        cursor: 0,
        executed: false,
        fetch: None,
        bound: Vec::new(),
        error: ErrorState::default(),
        row_count: 0,
    })));
    if class.as_slice() != b"PDOStatement" {
        if let Some(m) = ctx.resolve_method(cid, b"__construct") {
            let callable = match &m.body {
                rphp_runtime::MethodBody::User(f) => rphp_runtime::Callable::User {
                    func: f.clone(),
                    this: Some(obj.clone()),
                    scope: Some(m.decl),
                    static_class: Some(cid),
                    closure: None,
                },
                rphp_runtime::MethodBody::Native(_) => rphp_runtime::Callable::NativeMethod {
                    method: m.clone(),
                    this: Some(obj.clone()),
                },
            };
            ctx.call_resolved(callable, &ctor_args)?;
        }
    }
    Ok(Ok(obj))
}

/// The parameter a `bindValue`/`bindParam`/`execute` key names: an int is
/// a 1-based position, a string a name (`:name`, the colon added when
/// missing).
fn param_key(v: &Value) -> ParamKey {
    match v.deref().into_owned() {
        Value::Int(i) => ParamKey::Pos(i.max(0) as usize),
        other => {
            let s = String::from_utf8_lossy(&other.to_php_bytes()).into_owned();
            if s.starts_with(':') {
                ParamKey::Name(s)
            } else {
                ParamKey::Name(format!(":{s}"))
            }
        }
    }
}

/// `PDOStatement::execute(?array $params = null): bool`, shared with
/// `PDO::query()`.
pub fn execute(ctx: &mut Ctx, obj: &Object, params: Option<&Array>, who: &str) -> Result<bool, Unwind> {
    let pdo = with_stmt(obj, |st| st.pdo.clone())?;
    clear_error(&pdo, Some(obj))?;
    // The bound values: `execute()`'s array replaces every earlier
    // binding; a `bindParam` cell is read now.
    let bound: Vec<(ParamKey, SqlValue)> = match params {
        Some(a) => {
            let mut out = Vec::new();
            for (k, v) in a.iter() {
                let key = match k {
                    ArrayKey::Int(i) => ParamKey::Pos((*i + 1).max(0) as usize),
                    ArrayKey::Str(s) => param_key(&Value::string(s)),
                };
                out.push((key, param_value(v, PARAM_STR)));
            }
            out
        }
        None => with_stmt(obj, |st| {
            st.params
                .iter()
                .map(|p| (p.key.clone(), param_value(&p.value, p.ty)))
                .collect()
        })?,
    };
    let sql = with_stmt(obj, |st| st.sql.clone())?;
    let result = with_conn(ctx, &pdo, |st| st.conn.run(&sql, &bound))?;
    match result {
        Ok(rs) => {
            with_stmt(obj, |st| {
                // php's sqlite driver reads `sqlite3_changes()` only when the
                // first step finds no row: a statement with rows counts 0.
                st.row_count = if rs.rows.is_empty() { rs.changes } else { 0 };
                st.columns = rs.columns;
                st.rows = rs.rows;
                st.cursor = 0;
                st.executed = true;
            })?;
            Ok(true)
        }
        Err(e) => {
            report(ctx, &pdo, Some(obj), who, &e)?;
            Ok(false)
        }
    }
}

fn stmt_execute(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let obj = this(o)?;
    let params = match args.first().map(|v| v.deref().into_owned()) {
        Some(Value::Array(a)) => Some(a),
        _ => None,
    };
    let ok = execute(ctx, obj, params.as_ref(), "PDOStatement::execute")?;
    Ok(Value::Bool(ok))
}

/// `bindValue(string|int $param, mixed $value, int $type = PDO::PARAM_STR): bool`
fn stmt_bind_value(_: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let obj = this(o)?;
    let key = param_key(&args[0]);
    let value = args.get(1).map(|v| v.deref().into_owned()).unwrap_or(Value::Null);
    let ty = int_arg(args, 2, PARAM_STR);
    with_stmt(obj, |st| {
        st.params.retain(|p| p.key != key);
        st.params.push(Param { key, value, ty });
    })?;
    Ok(Value::Bool(true))
}

/// `bindParam(string|int $param, mixed &$var, int $type = PDO::PARAM_STR, ...): bool`
/// — the variable's cell is read when the statement executes.
fn stmt_bind_param(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let obj = this(o)?;
    let key = param_key(&args[0]);
    let value = match ctx.ref_arg(1) {
        Some(cell) => Value::Ref(cell),
        None => args.get(1).cloned().unwrap_or(Value::Null),
    };
    let ty = int_arg(args, 2, PARAM_STR);
    with_stmt(obj, |st| {
        st.params.retain(|p| p.key != key);
        st.params.push(Param { key, value, ty });
    })?;
    Ok(Value::Bool(true))
}

/// `bindColumn(string|int $column, mixed &$var, int $type = PDO::PARAM_STR, ...): bool`
fn stmt_bind_column(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let obj = this(o)?;
    let key = match args[0].deref().into_owned() {
        Value::Int(i) => ParamKey::Pos(i.max(0) as usize),
        other => ParamKey::Name(String::from_utf8_lossy(&other.to_php_bytes()).into_owned()),
    };
    let Some(cell) = ctx.ref_arg(1) else {
        return Ok(Value::Bool(false));
    };
    let ty = int_arg(args, 2, PARAM_STR);
    with_stmt(obj, |st| {
        st.bound.retain(|b| b.key != key);
        st.bound.push(BoundColumn { key, cell, ty });
    })?;
    Ok(Value::Bool(true))
}

// ---- values ----------------------------------------------------------------------

/// A column value as php hands it out: SQLite's own types, or strings
/// under `ATTR_STRINGIFY_FETCHES`, nulls per `ATTR_ORACLE_NULLS`.
fn php_value(v: &SqlValue, stringify: bool, oracle_nulls: i64) -> Value {
    match v {
        SqlValue::Null => match oracle_nulls {
            NULL_TO_STRING => Value::string(b""),
            _ => Value::Null,
        },
        SqlValue::Int(i) => {
            if stringify {
                Value::string(i.to_string().as_bytes())
            } else {
                Value::Int(*i)
            }
        }
        SqlValue::Float(f) => {
            if stringify {
                Value::string(&Value::Float(*f).to_php_bytes())
            } else {
                Value::Float(*f)
            }
        }
        SqlValue::Text(t) | SqlValue::Blob(t) => {
            if t.is_empty() && oracle_nulls == NULL_EMPTY_STRING {
                Value::Null
            } else {
                Value::string(t)
            }
        }
    }
}

/// A column's name as `ATTR_CASE` spells it.
fn column_name(name: &str, case: i64) -> Vec<u8> {
    match case {
        CASE_LOWER => name.to_ascii_lowercase().into_bytes(),
        CASE_UPPER => name.to_ascii_uppercase().into_bytes(),
        _ => name.as_bytes().to_vec(),
    }
}

/// The next row's values, php-typed, with the column names; `None` at the
/// end. Bound columns are filled on the way.
fn next_row(obj: &Object) -> Result<Option<(Vec<Vec<u8>>, Vec<Value>)>, Unwind> {
    let (pdo, done) = with_stmt(obj, |st| (st.pdo.clone(), !st.executed || st.cursor >= st.rows.len()))?;
    if done {
        return Ok(None);
    }
    let (stringify, oracle_nulls, case) = with_pdo(&pdo, |st| (st.attrs.stringify, st.attrs.oracle_nulls, st.attrs.case))?;
    let row = with_stmt(obj, |st| {
        let raw = &st.rows[st.cursor];
        st.cursor += 1;
        let names: Vec<Vec<u8>> = st.columns.iter().map(|c| column_name(&c.name, case)).collect();
        let values: Vec<Value> = raw.iter().map(|v| php_value(v, stringify, oracle_nulls)).collect();
        // Bound columns.
        for b in &st.bound {
            let idx = match &b.key {
                ParamKey::Pos(p) => p.checked_sub(1),
                ParamKey::Name(n) => st.columns.iter().position(|c| c.name == *n),
            };
            if let Some(i) = idx {
                if let Some(v) = values.get(i) {
                    let v = match b.ty {
                        PARAM_INT => Value::Int(v.to_int()),
                        PARAM_STR if !matches!(v, Value::Null) => Value::string(&v.to_php_bytes()),
                        _ => v.clone(),
                    };
                    b.cell.set(v);
                }
            }
        }
        (names, values)
    })?;
    Ok(Some(row))
}

fn assoc_row(names: &[Vec<u8>], values: &[Value]) -> Array {
    let mut a = Array::new();
    for (n, v) in names.iter().zip(values) {
        a.set(ArrayKey::str(n), v.clone());
    }
    a
}

fn num_row(values: &[Value]) -> Array {
    let mut a = Array::new();
    for v in values {
        a.push(v.clone());
    }
    a
}

/// `FETCH_BOTH`: the numeric keys are the columns' positions (`offset`
/// on, when the first column was consumed as a group key).
fn both_row(names: &[Vec<u8>], values: &[Value], offset: usize) -> Array {
    let mut a = Array::new();
    for (i, (n, v)) in names.iter().zip(values).enumerate() {
        a.set(ArrayKey::str(n), v.clone());
        a.set(ArrayKey::Int((i + offset) as i64), v.clone());
    }
    a
}

/// `FETCH_NAMED`: a repeated column name collects its values in an array.
fn named_row(names: &[Vec<u8>], values: &[Value]) -> Array {
    let mut a = Array::new();
    for (n, v) in names.iter().zip(values) {
        let key = ArrayKey::str(n);
        match a.get_deref(&key) {
            Some(Value::Array(mut existing)) => {
                existing.push(v.clone());
                a.set(key, Value::Array(existing));
            }
            Some(prev) => {
                let mut list = Array::new();
                list.push(prev);
                list.push(v.clone());
                a.set(key, Value::Array(list));
            }
            None => a.set(key, v.clone()),
        }
    }
    a
}

/// A `stdClass` with the row's columns as properties.
fn obj_row(ctx: &mut Ctx, names: &[Vec<u8>], values: &[Value]) -> Value {
    let std = ctx.class_by_name(b"stdClass").expect("stdClass");
    let o = ctx.instantiate(std);
    for (n, v) in names.iter().zip(values) {
        o.dyn_set(n, v.clone());
    }
    Value::Object(o)
}

/// Set the row's columns as properties of `o` (through the property write
/// path, so declared properties are typed and `__set` runs).
fn fill_object(ctx: &mut Ctx, o: &Object, names: &[Vec<u8>], values: &[Value]) -> Result<(), Unwind> {
    let holder = Value::Object(o.clone());
    for (n, v) in names.iter().zip(values) {
        ctx.assign_prop(&holder, n, v.clone())?;
    }
    Ok(())
}

/// `new $class(...$ctor_args)` with the row's columns as properties, before
/// (default) or after (`FETCH_PROPS_LATE`) the constructor.
fn class_row(
    ctx: &mut Ctx,
    class: &[u8],
    ctor_args: &[Value],
    late: bool,
    names: &[Vec<u8>],
    values: &[Value],
) -> Result<Value, Unwind> {
    let cid = ctx.lookup_class_or_error(class)?;
    let o = ctx.new_object(cid)?;
    if !late {
        fill_object(ctx, &o, names, values)?;
    }
    if let Some(m) = ctx.resolve_method(cid, b"__construct") {
        let callable = match &m.body {
            rphp_runtime::MethodBody::User(f) => rphp_runtime::Callable::User {
                func: f.clone(),
                this: Some(o.clone()),
                scope: Some(m.decl),
                static_class: Some(cid),
                closure: None,
            },
            rphp_runtime::MethodBody::Native(_) => rphp_runtime::Callable::NativeMethod {
                method: m.clone(),
                this: Some(o.clone()),
            },
        };
        ctx.call_resolved(callable, ctor_args)?;
    } else if !ctor_args.is_empty() {
        return Err(Unwind::value_error(
            "User-supplied statement does not accept constructor arguments",
        ));
    }
    if late {
        fill_object(ctx, &o, names, values)?;
    }
    Ok(Value::Object(o))
}

/// The class and constructor arguments a `FETCH_CLASS` spec names.
fn class_spec(spec: &FetchSpec) -> (Vec<u8>, Vec<Value>) {
    let class = spec.args.first().map(|v| v.to_php_bytes()).unwrap_or_else(|| b"stdClass".to_vec());
    let ctor = match spec.args.get(1).map(|v| v.deref().into_owned()) {
        Some(Value::Array(a)) => a.iter().map(|(_, v)| v.deref().into_owned()).collect(),
        _ => Vec::new(),
    };
    (class, ctor)
}

/// The mode a fetch runs in: the explicit one, else the statement's, else
/// the connection's default.
fn effective(obj: &Object, mode: i64, args: &[Value]) -> Result<FetchSpec, Unwind> {
    if mode != FETCH_DEFAULT {
        return Ok(FetchSpec {
            mode,
            args: args.to_vec(),
        });
    }
    let (pdo, own) = with_stmt(obj, |st| (st.pdo.clone(), st.fetch.clone()))?;
    if let Some(f) = own {
        return Ok(f);
    }
    with_pdo(&pdo, |st| st.attrs.default_fetch.clone())
}

/// One row in `spec`'s mode, or `false` at the end.
fn fetch_one(ctx: &mut Ctx, obj: &Object, spec: &FetchSpec) -> NativeResult {
    let Some((names, values)) = next_row(obj)? else {
        return Ok(Value::Bool(false));
    };
    let mode = spec.mode & FETCH_MODE_MASK;
    let flags = spec.mode & !FETCH_MODE_MASK;
    Ok(match mode {
        FETCH_ASSOC => Value::Array(assoc_row(&names, &values)),
        FETCH_NUM => Value::Array(num_row(&values)),
        FETCH_BOTH | FETCH_DEFAULT => Value::Array(both_row(&names, &values, 0)),
        FETCH_NAMED => Value::Array(named_row(&names, &values)),
        FETCH_OBJ => obj_row(ctx, &names, &values),
        FETCH_LAZY => {
            let cid = ctx.class_by_name(b"PDORow").expect("PDORow");
            let o = ctx.instantiate(cid);
            let sql = with_stmt(obj, |st| st.sql.clone())?;
            o.set(b"queryString", Value::string(sql.as_bytes()));
            for (n, v) in names.iter().zip(&values) {
                o.dyn_set(n, v.clone());
            }
            Value::Object(o)
        }
        FETCH_BOUND => Value::Bool(true),
        FETCH_COLUMN => {
            let i = spec.args.first().map_or(0, |v| v.to_int());
            match values.get(i.max(0) as usize) {
                Some(v) if i >= 0 => v.clone(),
                _ => return Err(Unwind::value_error("Invalid column index")),
            }
        }
        FETCH_CLASS => {
            if flags & FETCH_CLASSTYPE != 0 {
                // The first column names the class; the rest are its data.
                let class = values.first().map(|v| v.to_php_bytes()).unwrap_or_default();
                let (_, ctor) = class_spec(spec);
                class_row(ctx, &class, &ctor, flags & FETCH_PROPS_LATE != 0, &names[1..], &values[1..])?
            } else {
                if spec.args.is_empty() {
                    return Err(general_error(ctx, obj, "No fetch class specified")?);
                }
                let (class, ctor) = class_spec(spec);
                class_row(ctx, &class, &ctor, flags & FETCH_PROPS_LATE != 0, &names, &values)?
            }
        }
        FETCH_INTO => {
            let Some(Value::Object(o)) = spec.args.first().map(|v| v.deref().into_owned()) else {
                return Err(general_error(ctx, obj, "No fetch-into object specified.")?);
            };
            fill_object(ctx, &o, &names, &values)?;
            Value::Object(o)
        }
        FETCH_KEY_PAIR => {
            if values.len() != 2 {
                return Err(general_error(
                    ctx,
                    obj,
                    "PDO::FETCH_KEY_PAIR fetch mode requires the result set to contain exactly 2 columns.",
                )?);
            }
            let mut a = Array::new();
            let key = rphp_value::array_key(&values[0]).unwrap_or(ArrayKey::Int(0));
            a.set(key, values[1].clone());
            Value::Array(a)
        }
        FETCH_FUNC => {
            return Err(Unwind::value_error(
                "PDOStatement::fetch(): Argument #1 ($mode) PDO::FETCH_FUNC can only be used with PDOStatement::fetchAll()",
            ))
        }
        _ => Value::Array(both_row(&names, &values, 0)),
    })
}

/// php's `pdo_raise_impl_error(HY000)` for a fetch that cannot proceed:
/// recorded on the statement and reported per the error mode; the `Ok`
/// is the exception to throw under `ERRMODE_EXCEPTION`, or php's plain
/// `Error` otherwise (a fetch has no `false` to fall back to here).
fn general_error(ctx: &mut Ctx, obj: &Object, message: &str) -> Result<Unwind, Unwind> {
    let pdo = with_stmt(obj, |st| st.pdo.clone())?;
    let err = DbError::new("HY000", 0, message);
    report(ctx, &pdo, Some(obj), "PDOStatement::fetch", &err)?;
    Ok(exception(ctx, &err))
}

/// `fetch(int $mode = PDO::FETCH_DEFAULT, int $cursorOrientation = PDO::FETCH_ORI_NEXT, int $cursorOffset = 0): mixed`
fn stmt_fetch(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let obj = this(o)?;
    let mode = int_arg(args, 0, FETCH_DEFAULT);
    let spec = effective(obj, mode, &[])?;
    fetch_one(ctx, obj, &spec)
}

/// `fetchObject(?string $class = "stdClass", array $constructorArgs = []): object|false`
fn stmt_fetch_object(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let obj = this(o)?;
    let class = match args.first().map(|v| v.deref().into_owned()) {
        Some(Value::Null) | None => Value::string(b"stdClass"),
        Some(v) => v,
    };
    let ctor = args.get(1).cloned().unwrap_or_else(Value::empty_array);
    fetch_one(
        ctx,
        obj,
        &FetchSpec {
            mode: FETCH_CLASS,
            args: vec![class, ctor],
        },
    )
}

/// `fetchColumn(int $column = 0): mixed`
fn stmt_fetch_column(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let obj = this(o)?;
    let col = int_arg(args, 0, 0);
    fetch_one(
        ctx,
        obj,
        &FetchSpec {
            mode: FETCH_COLUMN,
            args: vec![Value::Int(col)],
        },
    )
}

/// `fetchAll(int $mode = PDO::FETCH_DEFAULT, mixed ...$args): array`
fn stmt_fetch_all(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let obj = this(o)?;
    let mode = int_arg(args, 0, FETCH_DEFAULT);
    let rest: Vec<Value> = args.iter().skip(1).map(|v| v.deref().into_owned()).collect();
    let spec = effective(obj, mode, &rest)?;
    let base = spec.mode & FETCH_MODE_MASK;
    let flags = spec.mode & !FETCH_MODE_MASK;
    if base == FETCH_LAZY {
        return Err(Unwind::value_error(
            "PDOStatement::fetchAll(): Argument #1 ($mode) PDO::FETCH_LAZY cannot be used with PDOStatement::fetchAll()",
        ));
    }
    let mut out = Array::new();
    if base == FETCH_FUNC {
        let Some(callable) = spec.args.first().cloned() else {
            return Err(Unwind::argument_count_error(format!(
                "PDOStatement::fetchAll() expects exactly 2 argument for PDO::FETCH_FUNC, {} given",
                rest.len() + 1
            )));
        };
        while let Some((_, values)) = next_row(obj)? {
            out.push(ctx.call_value(&callable, &values)?);
        }
        return Ok(Value::Array(out));
    }
    if flags & (FETCH_GROUP | FETCH_UNIQUE) != 0 {
        // The first column keys the rest of the row (fetched in the base
        // mode); GROUP collects lists, UNIQUE keeps the last.
        let column_mode = base == FETCH_COLUMN;
        while let Some((names, values)) = next_row(obj)? {
            if values.is_empty() {
                continue;
            }
            let key = rphp_value::array_key(&values[0]).unwrap_or(ArrayKey::Int(0));
            let rest_names = &names[1..];
            let rest_values = &values[1..];
            let item = if column_mode {
                rest_values.first().cloned().unwrap_or(Value::Null)
            } else {
                match base {
                    FETCH_NUM => Value::Array(num_row(rest_values)),
                    FETCH_ASSOC => Value::Array(assoc_row(rest_names, rest_values)),
                    FETCH_OBJ => obj_row(ctx, rest_names, rest_values),
                    FETCH_CLASS => {
                        let (class, ctor) = class_spec(&spec);
                        class_row(ctx, &class, &ctor, flags & FETCH_PROPS_LATE != 0, rest_names, rest_values)?
                    }
                    _ => Value::Array(both_row(rest_names, rest_values, 1)),
                }
            };
            if flags & FETCH_UNIQUE != 0 {
                out.set(key, item);
            } else {
                match out.get_deref(&key) {
                    Some(Value::Array(mut list)) => {
                        list.push(item);
                        out.set(key, Value::Array(list));
                    }
                    _ => {
                        let mut list = Array::new();
                        list.push(item);
                        out.set(key, Value::Array(list));
                    }
                }
            }
        }
        return Ok(Value::Array(out));
    }
    if base == FETCH_KEY_PAIR {
        while let Some((_, values)) = next_row(obj)? {
            if values.len() != 2 {
                return Err(general_error(
                    ctx,
                    obj,
                    "PDO::FETCH_KEY_PAIR fetch mode requires the result set to contain exactly 2 columns.",
                )?);
            }
            let key = rphp_value::array_key(&values[0]).unwrap_or(ArrayKey::Int(0));
            out.set(key, values[1].clone());
        }
        return Ok(Value::Array(out));
    }
    let _ = FETCH_SERIALIZE;
    loop {
        let v = fetch_one(ctx, obj, &spec)?;
        if matches!(v, Value::Bool(false)) {
            break;
        }
        out.push(v);
    }
    Ok(Value::Array(out))
}

/// `setFetchMode(int $mode, mixed ...$args): bool`, shared with
/// `PDO::query()`'s trailing arguments.
pub fn set_fetch_mode(ctx: &mut Ctx, obj: &Object, mode: i64, args: &[Value], who: &str) -> Result<(), Unwind> {
    let base = mode & FETCH_MODE_MASK;
    // php counts the mode itself (and `PDO::query()`'s SQL) among the
    // arguments it expects.
    let lead = if who == "PDO::query" { 2 } else { 1 };
    let given = args.len() + lead;
    let count_error = |what: &str, n: usize| {
        let n = n + lead - 1;
        Unwind::argument_count_error(format!(
            "{who}() expects {what} {n} argument{} for the fetch mode provided, {given} given",
            if n == 1 { "" } else { "s" }
        ))
    };
    match base {
        FETCH_COLUMN => {
            if args.len() != 1 {
                return Err(count_error("exactly", 2));
            }
        }
        FETCH_CLASS => {
            if mode & FETCH_CLASSTYPE == 0 {
                if args.is_empty() {
                    return Err(count_error("at least", 2));
                }
                if args.len() > 2 {
                    return Err(count_error("at most", 3));
                }
                let name = args[0].to_php_bytes();
                if ctx.lookup_class(&name)?.is_none() {
                    return Err(Unwind::type_error(format!("{who}(): Argument #2 must be a valid class")));
                }
            } else if args.len() > 1 {
                return Err(count_error("at most", 2));
            }
        }
        FETCH_INTO => {
            if args.len() != 1 {
                return Err(count_error("exactly", 2));
            }
            if !matches!(args[0].deref().into_owned(), Value::Object(_)) {
                return Err(Unwind::type_error(format!(
                    "{who}(): Argument #2 must be of type object, {} given",
                    args[0].type_name()
                )));
            }
        }
        _ => {
            if !args.is_empty() {
                return Err(count_error("exactly", 1));
            }
        }
    }
    with_stmt(obj, |st| {
        st.fetch = Some(FetchSpec {
            mode,
            args: args.to_vec(),
        })
    })
}

fn stmt_set_fetch_mode(ctx: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let obj = this(o)?;
    let mode = int_arg(args, 0, FETCH_BOTH);
    let rest: Vec<Value> = args.iter().skip(1).map(|v| v.deref().into_owned()).collect();
    set_fetch_mode(ctx, obj, mode, &rest, "PDOStatement::setFetchMode")?;
    Ok(Value::Bool(true))
}

/// `rowCount(): int` — what the last change statement changed.
fn stmt_row_count(_: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let obj = this(o)?;
    Ok(Value::Int(with_stmt(obj, |st| st.row_count)?))
}

/// `columnCount(): int`
fn stmt_column_count(_: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let obj = this(o)?;
    Ok(Value::Int(with_stmt(obj, |st| st.columns.len() as i64)?))
}

/// `getColumnMeta(int $column): array|false`
fn stmt_get_column_meta(_: &mut Ctx, o: Option<&Object>, args: &mut [Value]) -> NativeResult {
    let obj = this(o)?;
    let i = int_arg(args, 0, 0);
    if i < 0 {
        return Err(Unwind::value_error(
            "PDOStatement::getColumnMeta(): Argument #1 ($column) must be greater than or equal to 0",
        ));
    }
    let col = with_stmt(obj, |st| st.columns.get(i as usize).cloned())?;
    let Some(col) = col else {
        return Ok(Value::Bool(false));
    };
    let mut a = Array::new();
    let native = col.native_type.unwrap_or("null");
    a.set(ArrayKey::str(b"native_type"), Value::string(native.as_bytes()));
    let pdo_type = match native {
        "integer" => PARAM_INT,
        "null" => PARAM_NULL,
        _ => PARAM_STR,
    };
    a.set(ArrayKey::str(b"pdo_type"), Value::Int(pdo_type));
    if let Some(d) = &col.decl_type {
        a.set(ArrayKey::str(b"sqlite:decl_type"), Value::string(d.as_bytes()));
    }
    if let Some(t) = &col.table {
        a.set(ArrayKey::str(b"table"), Value::string(t.as_bytes()));
    }
    a.set(ArrayKey::str(b"flags"), Value::empty_array());
    a.set(ArrayKey::str(b"name"), Value::string(col.name.as_bytes()));
    a.set(ArrayKey::str(b"len"), Value::Int(-1));
    a.set(ArrayKey::str(b"precision"), Value::Int(0));
    Ok(Value::Array(a))
}

/// `errorCode(): ?string`
fn stmt_error_code(_: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let obj = this(o)?;
    let s = with_stmt(obj, |st| st.error.sqlstate.clone())?;
    Ok(Value::string(s.as_bytes()))
}

/// `errorInfo(): array`
fn stmt_error_info(_: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let obj = this(o)?;
    with_stmt(obj, |st| st.error.info())
}

/// `closeCursor(): bool` — the rows are dropped; a fetch answers `false`
/// until the next `execute()`.
fn stmt_close_cursor(_: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let obj = this(o)?;
    with_stmt(obj, |st| {
        st.rows.clear();
        st.cursor = 0;
    })?;
    Ok(Value::Bool(true))
}

/// `nextRowset(): bool` — SQLite has no multi-rowset statements.
fn stmt_next_rowset(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let obj = this(o)?;
    let pdo = with_stmt(obj, |st| st.pdo.clone())?;
    let err = DbError::new("IM001", 0, "driver does not support multiple rowsets");
    report(ctx, &pdo, Some(obj), "PDOStatement::nextRowset", &err)?;
    Ok(Value::Bool(false))
}

/// `setAttribute(int $attribute, mixed $value): bool` / `getAttribute(int $name): mixed`
/// — SQLite statements have none.
fn stmt_set_attribute(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let obj = this(o)?;
    let pdo = with_stmt(obj, |st| st.pdo.clone())?;
    let _ = (ctx, pdo, obj);
    Ok(Value::Bool(false))
}

fn stmt_get_attribute(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let obj = this(o)?;
    let pdo = with_stmt(obj, |st| st.pdo.clone())?;
    let err = DbError::new("IM001", 0, "driver doesn't support getting that attribute");
    report(ctx, &pdo, Some(obj), "PDOStatement::getAttribute", &err)?;
    Ok(Value::Bool(false))
}

/// `debugDumpParams(): ?bool` — php's dump, printed.
fn stmt_debug_dump_params(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let obj = this(o)?;
    let (sql, params) = with_stmt(obj, |st| (st.sql.clone(), st.params.clone()))?;
    let mut out = format!("SQL: [{}] {}\nParams:  {}\n", sql.len(), sql, params.len());
    for p in &params {
        match &p.key {
            ParamKey::Name(n) => {
                out.push_str(&format!(
                    "Key: Name: [{}] {n}\nparamno=-1\nname=[{}] \"{n}\"\nis_param=1\nparam_type={}\n",
                    n.len(),
                    n.len(),
                    p.ty
                ));
            }
            ParamKey::Pos(i) => {
                let zero = i.saturating_sub(1);
                out.push_str(&format!(
                    "Key: Position #{zero}:\nparamno={zero}\nname=[0] \"\"\nis_param=1\nparam_type={}\n",
                    p.ty
                ));
            }
        }
    }
    ctx.echo(out.as_bytes());
    Ok(Value::Bool(true))
}

/// `getIterator(): Iterator` — every remaining row in the statement's
/// fetch mode, as an `InternalIterator` (the rows are read now).
fn stmt_get_iterator(ctx: &mut Ctx, o: Option<&Object>, _: &mut [Value]) -> NativeResult {
    let obj = this(o)?;
    let spec = effective(obj, FETCH_DEFAULT, &[])?;
    let mut items: Vec<(Value, Value)> = Vec::new();
    let mut i = 0i64;
    loop {
        let v = fetch_one(ctx, obj, &spec)?;
        if matches!(v, Value::Bool(false)) {
            break;
        }
        items.push((Value::Int(i), v));
        i += 1;
    }
    Ok(Value::Object(rphp_stdlib::new_internal_iterator(ctx, items)))
}

/// Register `PDOStatement` and `PDORow`.
pub fn register(r: &mut Registry) {
    r.class("PDOStatement")
        .implements(&["IteratorAggregate"])
        .prop("queryString", Visibility::Public, Value::string(b""))
        .method("execute", nm!(0, Some(1), stmt_execute))
        .method("fetch", nm!(0, Some(3), stmt_fetch))
        .method("fetchAll", nm!(0, None, stmt_fetch_all))
        .method("fetchColumn", nm!(0, Some(1), stmt_fetch_column))
        .method("fetchObject", nm!(0, Some(2), stmt_fetch_object))
        .method("bindValue", nm!(2, Some(3), stmt_bind_value))
        .method("bindParam", method_ref(2, Some(5), 0b10, stmt_bind_param))
        .method("bindColumn", method_ref(2, Some(5), 0b10, stmt_bind_column))
        .method("rowCount", nm!(0, Some(0), stmt_row_count))
        .method("columnCount", nm!(0, Some(0), stmt_column_count))
        .method("getColumnMeta", nm!(1, Some(1), stmt_get_column_meta))
        .method("setFetchMode", nm!(1, None, stmt_set_fetch_mode))
        .method("errorCode", nm!(0, Some(0), stmt_error_code))
        .method("errorInfo", nm!(0, Some(0), stmt_error_info))
        .method("closeCursor", nm!(0, Some(0), stmt_close_cursor))
        .method("nextRowset", nm!(0, Some(0), stmt_next_rowset))
        .method("setAttribute", nm!(2, Some(2), stmt_set_attribute))
        .method("getAttribute", nm!(1, Some(1), stmt_get_attribute))
        .method("debugDumpParams", nm!(0, Some(0), stmt_debug_dump_params))
        .method("getIterator", nm!(0, Some(0), stmt_get_iterator))
        .finish();
    r.class("PDORow")
        .prop("queryString", Visibility::Public, Value::string(b""))
        .finish();
    let _ = (ATTR_STRINGIFY_FETCHES, str_arg, exception, pdo::PARAM_BOOL);
}
