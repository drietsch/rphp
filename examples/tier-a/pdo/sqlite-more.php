<?php
// The rest of PDO's surface over a file database: persistence across
// connections, the connection attributes that shape fetched values, the
// statement class option, subclassing PDO, and what a broken callback does.
$path = sys_get_temp_dir() . '/rphp-pdo-' . getmypid() . '.sqlite';
@unlink($path);
$db = new PDO("sqlite:$path", null, null, [PDO::ATTR_ERRMODE => PDO::ERRMODE_EXCEPTION, PDO::ATTR_DEFAULT_FETCH_MODE => PDO::FETCH_ASSOC]);
var_dump($db->getAttribute(PDO::ATTR_DEFAULT_FETCH_MODE) === PDO::FETCH_ASSOC, is_file($path));
$db->exec("CREATE TABLE people (id INTEGER PRIMARY KEY AUTOINCREMENT, Name TEXT NOT NULL, age INT, note TEXT)");
$ins = $db->prepare("INSERT INTO people (Name, age, note) VALUES (:name, :age, :note)");
foreach ([['Ann', 31, null], ['Bob', null, ''], ['Cy', 45, 'x']] as [$n, $a, $note]) {
    $ins->bindValue(':name', $n);
    $ins->bindValue(':age', $a, $a === null ? PDO::PARAM_NULL : PDO::PARAM_INT);
    $ins->bindValue(':note', $note);
    var_dump($ins->execute(), $ins->rowCount(), $db->lastInsertId());
}
unset($db);
$db = new PDO("sqlite:$path");
var_dump($db->query("SELECT * FROM people ORDER BY id")->fetch());
$db->setAttribute(PDO::ATTR_CASE, PDO::CASE_LOWER);
var_dump($db->query("SELECT Name, age FROM people WHERE id = 1")->fetch(PDO::FETCH_ASSOC));
$db->setAttribute(PDO::ATTR_CASE, PDO::CASE_UPPER);
var_dump($db->query("SELECT Name, age FROM people WHERE id = 1")->fetch(PDO::FETCH_OBJ));
$db->setAttribute(PDO::ATTR_CASE, PDO::CASE_NATURAL);
foreach ([PDO::NULL_NATURAL, PDO::NULL_EMPTY_STRING, PDO::NULL_TO_STRING] as $mode) {
    $db->setAttribute(PDO::ATTR_ORACLE_NULLS, $mode);
    var_dump($db->query("SELECT age, note FROM people WHERE id = 2")->fetch(PDO::FETCH_NUM));
}
$db->setAttribute(PDO::ATTR_ORACLE_NULLS, PDO::NULL_NATURAL);
var_dump($db->getAttribute(PDO::ATTR_CASE), $db->getAttribute(PDO::ATTR_ORACLE_NULLS), $db->getAttribute(PDO::ATTR_STATEMENT_CLASS));
echo "--- statement class ---\n";
class MyStmt extends PDOStatement {
    public $tag;
    protected function __construct($tag = 'none') { $this->tag = $tag; }
    public function firstName() { return $this->fetchColumn(); }
}
$db->setAttribute(PDO::ATTR_STATEMENT_CLASS, [MyStmt::class, ['tagged']]);
$st = $db->query("SELECT Name FROM people ORDER BY id");
var_dump(get_class($st), $st->tag, $st->firstName(), $db->getAttribute(PDO::ATTR_STATEMENT_CLASS));
try { $db->setAttribute(PDO::ATTR_STATEMENT_CLASS, ['stdClass']); } catch (Throwable $e) { echo get_class($e), ': ', $e->getMessage(), "\n"; }
$db->setAttribute(PDO::ATTR_STATEMENT_CLASS, ['PDOStatement']);
echo "--- subclassing PDO ---\n";
class MyPdo extends PDO {
    public int $queries = 0;
    public function query(string $q, ?int $m = null, mixed ...$a): PDOStatement|false { $this->queries++; return parent::query($q, $m, ...$a); }
}
$my = new MyPdo("sqlite:$path");
$my->query("SELECT 1"); $my->query("SELECT 2");
var_dump($my->queries, $my instanceof PDO, get_class($my->prepare("SELECT 1")));
echo "--- fetch details ---\n";
$st = $db->query("SELECT id, Name, age FROM people ORDER BY id");
var_dump($st->fetch(PDO::FETCH_NUM), $st->fetchColumn(1), $st->fetch(PDO::FETCH_ASSOC), $st->fetch(), $st->fetchColumn());
var_dump($db->query("SELECT Name AS n, Name AS n FROM people WHERE id = 1")->fetch(PDO::FETCH_NAMED));
var_dump($db->query("SELECT Name AS n, Name AS n FROM people WHERE id = 1")->fetch(PDO::FETCH_ASSOC));
var_dump($db->query("SELECT id, Name FROM people ORDER BY id")->fetchAll(PDO::FETCH_UNIQUE | PDO::FETCH_COLUMN));
var_dump($db->query("SELECT age, Name FROM people ORDER BY id")->fetchAll(PDO::FETCH_GROUP | PDO::FETCH_ASSOC));
var_dump($db->query("SELECT 'Row' AS cls, 1 AS id, 'zz' AS name FROM people WHERE id = 1")->fetchAll(PDO::FETCH_CLASS | PDO::FETCH_CLASSTYPE));
class Row { public $id; public $name; }
$st = $db->prepare("SELECT id, Name AS name FROM people WHERE id = ?");
$st->setFetchMode(PDO::FETCH_CLASS, 'Row');
$st->execute([3]);
var_dump($st->fetch());
try { $st->setFetchMode(PDO::FETCH_COLUMN); } catch (Throwable $e) { echo get_class($e), ': ', $e->getMessage(), "\n"; }
try { $db->query("SELECT 1, 2, 3")->fetchAll(PDO::FETCH_KEY_PAIR); } catch (Throwable $e) { echo get_class($e), ': ', $e->getMessage(), "\n"; }
try { $db->query("SELECT 1")->fetch(PDO::FETCH_FUNC); } catch (Throwable $e) { echo get_class($e), ': ', $e->getMessage(), "\n"; }
echo "--- callbacks that throw ---\n";
$sq = Pdo\Sqlite::connect("sqlite:$path");
$sq->createFunction('boom', function ($x) { throw new RuntimeException("udf $x"); }, 1);
try { $sq->query("SELECT boom(Name) FROM people")->fetchAll(); } catch (Throwable $e) { echo get_class($e), ': ', $e->getMessage(), "\n"; }
var_dump($sq->query("SELECT count(*) FROM people")->fetchColumn());
echo "--- ini and pragmas ---\n";
var_dump($db->query("PRAGMA table_info(people)")->fetchAll(PDO::FETCH_COLUMN, 1));
var_dump($db->exec("PRAGMA journal_mode = MEMORY"), $db->query("PRAGMA journal_mode")->fetchColumn());
var_dump($db->query("SELECT sqlite_version() = sqlite_version()")->fetchColumn(), $db->query("SELECT typeof(?)")->fetchColumn());
$st = $db->prepare("SELECT ?"); $st->execute(); var_dump($st->fetchColumn());
var_dump($db->exec("DROP TABLE people"), $db->query("SELECT name FROM sqlite_master")->fetchAll(PDO::FETCH_COLUMN));
unset($db, $my, $sq, $st, $ins);
var_dump(unlink($path));
