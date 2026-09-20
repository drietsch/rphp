<?php
// PDO over SQLite: connections, statements, every fetch mode, parameter
// typing, php's error modes and their texts, transactions, the driver's
// own subclass and its user-defined functions.
$db = new PDO('sqlite::memory:');
var_dump(get_class($db), $db instanceof PDO, $db->getAttribute(PDO::ATTR_ERRMODE) === PDO::ERRMODE_EXCEPTION, $db->getAttribute(PDO::ATTR_DRIVER_NAME), $db->getAttribute(PDO::ATTR_CASE), $db->getAttribute(PDO::ATTR_ORACLE_NULLS), $db->getAttribute(PDO::ATTR_STRINGIFY_FETCHES), $db->getAttribute(PDO::ATTR_DEFAULT_FETCH_MODE) === PDO::FETCH_BOTH, is_string($db->getAttribute(PDO::ATTR_SERVER_VERSION)), $db->getAttribute(PDO::ATTR_CLIENT_VERSION) === $db->getAttribute(PDO::ATTR_SERVER_VERSION));
try { $db->getAttribute(PDO::ATTR_EMULATE_PREPARES); } catch (PDOException $e) { var_dump($e->getMessage(), $e->getCode()); }
var_dump($db->exec("CREATE TABLE t (id INTEGER PRIMARY KEY, name TEXT, score REAL, data BLOB, n INT)"));
var_dump($db->exec("INSERT INTO t (name, score, data, n) VALUES ('a', 1.5, x'00ff', 7), ('b', 2, NULL, NULL)"));
var_dump($db->lastInsertId(), $db->lastInsertId('t'));
$st = $db->query("SELECT * FROM t ORDER BY id");
var_dump(get_class($st), $st->queryString, $st->columnCount(), $st->rowCount());
var_dump($st->fetch());
var_dump($st->fetch(PDO::FETCH_ASSOC), $st->fetch(), $st->fetch(PDO::FETCH_ASSOC));
foreach ([PDO::FETCH_ASSOC, PDO::FETCH_NUM, PDO::FETCH_BOTH, PDO::FETCH_OBJ, PDO::FETCH_COLUMN, PDO::FETCH_KEY_PAIR, PDO::FETCH_NAMED] as $mode) {
    $st = $db->query("SELECT id, name FROM t ORDER BY id");
    var_dump($st->fetchAll($mode));
}
var_dump($db->query("SELECT name, id FROM t")->fetchAll(PDO::FETCH_COLUMN, 1));
var_dump($db->query("SELECT name, id, score FROM t")->fetchAll(PDO::FETCH_GROUP));
var_dump($db->query("SELECT name, id, score FROM t")->fetchAll(PDO::FETCH_UNIQUE));
var_dump($db->query("SELECT name, id FROM t")->fetchAll(PDO::FETCH_GROUP | PDO::FETCH_COLUMN));
var_dump($db->query("SELECT id, name FROM t")->fetchAll(PDO::FETCH_FUNC, fn($id, $name) => "$id:$name"));
class Row { public $id; public $name; public $extra = 'x'; function __construct($a = 'none') { $this->extra = $a; } }
var_dump($db->query("SELECT id, name FROM t")->fetchAll(PDO::FETCH_CLASS, 'Row'));
var_dump($db->query("SELECT id, name FROM t")->fetchAll(PDO::FETCH_CLASS, 'Row', ['ctor']));
var_dump($db->query("SELECT id, name FROM t")->fetchAll(PDO::FETCH_CLASS | PDO::FETCH_PROPS_LATE, 'Row', ['late']));
var_dump($db->query("SELECT id, name FROM t")->fetchObject('Row', ['obj']));
var_dump($db->query("SELECT id, name FROM t")->fetchObject());
var_dump($db->query("SELECT name FROM t")->fetchColumn(), $db->query("SELECT id, name FROM t")->fetchColumn(1), $db->query("SELECT name FROM t WHERE 0")->fetchColumn());
$st = $db->query("SELECT id, name FROM t ORDER BY id");
$r = new Row; $st->setFetchMode(PDO::FETCH_INTO, $r); $x = $st->fetch(); var_dump($x === $r, $r);
echo "--- prepared ---\n";
$st = $db->prepare("SELECT * FROM t WHERE id = :id AND name = :name");
var_dump($st->execute([':id' => 1, ':name' => 'a']), $st->fetch(PDO::FETCH_ASSOC));
var_dump($st->execute(['id' => 2, 'name' => 'b']), $st->fetch(PDO::FETCH_ASSOC));
$st = $db->prepare("SELECT id FROM t WHERE id > ? AND name != ?");
var_dump($st->execute([0, 'zzz']), $st->fetchAll(PDO::FETCH_COLUMN));
$st = $db->prepare("INSERT INTO t (name, score, n) VALUES (:n, :s, :i)");
$st->bindValue(':n', 'c'); $st->bindValue(':s', 3.25); $st->bindValue(':i', 42, PDO::PARAM_INT);
var_dump($st->execute(), $st->rowCount(), $db->lastInsertId());
$name = 'd'; $st = $db->prepare("INSERT INTO t (name) VALUES (?)"); $st->bindParam(1, $name); $st->execute(); $name = 'e'; $st->execute();
var_dump($db->query("SELECT name FROM t WHERE id > 2 ORDER BY id")->fetchAll(PDO::FETCH_COLUMN));
$st = $db->prepare("SELECT id, name FROM t WHERE id = ?"); $st->execute([1]); $st->bindColumn(1, $cid); $st->bindColumn('name', $cname); $st->fetch(PDO::FETCH_BOUND); var_dump($cid, $cname);
echo "--- types ---\n";
var_dump($db->query("SELECT 1, 1.5, 'x', NULL, x'0102', 9223372036854775807, 1e20, typeof(1)")->fetch(PDO::FETCH_NUM));
$db->setAttribute(PDO::ATTR_STRINGIFY_FETCHES, true);
var_dump($db->query("SELECT 1, 1.5, NULL, 2e0")->fetch(PDO::FETCH_NUM));
$db->setAttribute(PDO::ATTR_STRINGIFY_FETCHES, false);
$st = $db->prepare("SELECT ? AS v, typeof(?) AS t"); 
foreach ([[1, PDO::PARAM_INT], ['1', PDO::PARAM_STR], [1.5, PDO::PARAM_STR], [true, PDO::PARAM_BOOL], [null, PDO::PARAM_NULL], ['5', PDO::PARAM_INT], [5, PDO::PARAM_STR], [false, PDO::PARAM_INT]] as [$v, $t]) { $st->bindValue(1, $v, $t); $st->bindValue(2, $v, $t); $st->execute(); var_dump($st->fetch(PDO::FETCH_ASSOC)); }
var_dump($db->quote("it's \"q\" x"), $db->quote(5), $db->quote(null), $db->quote("a", PDO::PARAM_INT));
try { $db->quote("a\0b"); } catch (PDOException $e) { var_dump($e->getMessage()); }
echo "--- errors ---\n";
try { $db->query("SELECT * FROM nope"); } catch (PDOException $e) { var_dump(get_class($e), $e->getMessage(), $e->getCode(), $e->errorInfo); }
try { $db->exec("INSERT INTO t (id, name) VALUES (1, 'dup')"); } catch (PDOException $e) { var_dump($e->getMessage(), $e->getCode(), $e->errorInfo); }
try { $db->prepare("SELECT * FROM t WHERE")->execute(); } catch (PDOException $e) { var_dump($e->getMessage()); }
try { $st = $db->prepare("SELECT ? , ?"); $st->execute([1]); } catch (PDOException $e) { var_dump($e->getMessage(), $e->getCode()); }
try { $st = $db->prepare("SELECT :a"); $st->execute([':b' => 1]); } catch (PDOException $e) { var_dump($e->getMessage()); }
var_dump($db->errorCode(), $db->errorInfo());
$db->setAttribute(PDO::ATTR_ERRMODE, PDO::ERRMODE_SILENT);
var_dump($db->query("SELECT * FROM nope"), $db->errorCode(), $db->errorInfo(), $db->exec("bad sql"));
$st = $db->prepare("SELECT * FROM t WHERE id = ?"); var_dump($st->execute(['x', 'y']), $st->errorCode(), $st->errorInfo());
$db->setAttribute(PDO::ATTR_ERRMODE, PDO::ERRMODE_WARNING);
var_dump(@$db->query("SELECT * FROM nope"));
$db->query("SELECT * FROM nope2");
$db->setAttribute(PDO::ATTR_ERRMODE, PDO::ERRMODE_EXCEPTION);
try { new PDO('sqlite:/nonexistent/dir/x.db'); } catch (PDOException $e) { var_dump($e->getMessage(), $e->getCode()); }
try { new PDO('nope:x'); } catch (PDOException $e) { var_dump($e->getMessage(), $e->getCode()); }
try { new PDO('garbage'); } catch (PDOException $e) { var_dump($e->getMessage(), $e->getCode()); }
echo "--- transactions ---\n";
var_dump($db->inTransaction(), $db->beginTransaction(), $db->inTransaction());
$db->exec("INSERT INTO t (name) VALUES ('tx')");
var_dump($db->rollBack(), $db->inTransaction(), $db->query("SELECT count(*) FROM t")->fetchColumn());
try { $db->commit(); } catch (PDOException $e) { var_dump($e->getMessage()); }
try { $db->rollBack(); } catch (PDOException $e) { var_dump($e->getMessage()); }
$db->beginTransaction(); try { $db->beginTransaction(); } catch (PDOException $e) { var_dump($e->getMessage()); } $db->exec("INSERT INTO t (name) VALUES ('tx2')"); var_dump($db->commit(), $db->query("SELECT count(*) FROM t")->fetchColumn());
echo "--- iteration & misc ---\n";
$st = $db->query("SELECT id, name FROM t WHERE id <= 2 ORDER BY id");
foreach ($st as $k => $row) { echo $k, ":", json_encode($row), "\n"; }
var_dump($st instanceof Traversable, $st instanceof IteratorAggregate, $db->query("SELECT id FROM t WHERE id = 1")->getColumnMeta(0));
var_dump(PDO::getAvailableDrivers() === PDO::getAvailableDrivers(), in_array('sqlite', PDO::getAvailableDrivers()));
$st = $db->prepare("SELECT :a, :b, ?"); 
$st->bindValue(':a', 1); $st->bindValue(':b', 's'); $st->bindValue(3, null);
ob_start(); $st->debugDumpParams(); $dd = ob_get_clean(); var_dump($dd);
var_dump($db->exec("UPDATE t SET n = 1 WHERE id > 100"), $db->exec("DELETE FROM t WHERE name = 'tx2'"), $db->exec("SELECT 1"));
var_dump($db->query("SELECT count(*) FROM t")->fetch()[0]);
$sq = Pdo\Sqlite::connect('sqlite::memory:');
var_dump(get_class($sq), $sq instanceof PDO, get_class(PDO::connect('sqlite::memory:')));
$db->sqliteCreateFunction('twice', fn($x) => is_numeric($x) ? $x * 2 : "$x$x", 1);
var_dump($db->query("SELECT twice(21), twice(1.5), twice('a'), twice(NULL)")->fetch(PDO::FETCH_NUM));
$db->sqliteCreateFunction('strrev_udf', 'strrev');
$sq->createFunction('plus', fn($a, $b) => $a + $b, 2, Pdo\Sqlite::DETERMINISTIC);
var_dump($sq->query("SELECT plus(1, 2), plus(1.5, 1)")->fetch(PDO::FETCH_NUM));
try { $sq->query("SELECT plus(1)"); } catch (PDOException $e) { var_dump($e->getMessage()); }
$sq->createAggregate('total', fn($ctx, $n, $v) => $ctx + $v, fn($ctx, $n) => $ctx * 10, 1);
$sq->exec("CREATE TABLE v (x INT)"); $sq->exec("INSERT INTO v VALUES (1), (2), (3)");
var_dump($sq->query("SELECT total(x) FROM v")->fetchColumn());
$sq->createCollation('rev', fn($a, $b) => strcmp($b, $a));
$sq->exec("CREATE TABLE w (s TEXT)"); $sq->exec("INSERT INTO w VALUES ('a'), ('c'), ('b')");
var_dump($sq->query("SELECT s FROM w ORDER BY s COLLATE rev")->fetchAll(PDO::FETCH_COLUMN));
var_dump($db->query("SELECT strrev_udf(name) FROM t WHERE id = 1")->fetchColumn());
var_dump($db->query("SELECT id FROM t ORDER BY id LIMIT 1")->fetch(PDO::FETCH_ASSOC), $db->query("SELECT id FROM t WHERE 0")->fetch(), $db->query("SELECT id FROM t WHERE 0")->fetchAll());
$st = $db->query("SELECT id FROM t ORDER BY id"); var_dump($st->fetch(PDO::FETCH_NUM), $st->closeCursor(), $st->fetch());
var_dump(PDO::PARAM_INT, PDO::PARAM_STR, PDO::PARAM_LOB, PDO::PARAM_NULL, PDO::PARAM_BOOL, PDO::FETCH_ASSOC, PDO::FETCH_BOTH, PDO::FETCH_LAZY, PDO::ATTR_ERRMODE, PDO::ERRMODE_EXCEPTION, PDO::FETCH_CLASS, PDO::FETCH_PROPS_LATE, PDO::ATTR_DEFAULT_FETCH_MODE, PDO::ATTR_TIMEOUT, PDO::SQLITE_ATTR_OPEN_FLAGS ?? null, PDO::SQLITE_OPEN_READONLY, PDO::SQLITE_DETERMINISTIC, PDO::ATTR_PERSISTENT, PDO::CASE_LOWER, PDO::NULL_NATURAL, PDO::FETCH_ORI_NEXT, PDO::CURSOR_FWDONLY, PDO::ATTR_CURSOR, PDO::ERR_NONE, PDO::PARAM_INPUT_OUTPUT, PDO::FETCH_SERIALIZE, PDO::FETCH_DEFAULT ?? null);
