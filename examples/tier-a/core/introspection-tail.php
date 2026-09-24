<?php
// get_included_files / get_required_files, get_declared_traits,
// get_mangled_object_vars, get_resource_id.
@mkdir('introspection_tail_dir');
file_put_contents('introspection_tail_dir/a.php', "<?php return 'a';\n");
file_put_contents('introspection_tail_dir/b.php', "<?php require_once __DIR__ . '/a.php'; return 'b';\n");

var_dump(array_map('basename', get_included_files()));
var_dump(include 'introspection_tail_dir/a.php');
var_dump(require 'introspection_tail_dir/b.php');
var_dump(include_once 'introspection_tail_dir/a.php');
var_dump(require_once __FILE__);
var_dump(@include 'introspection_tail_dir/missing.php');
var_dump(array_map('basename', get_included_files()));
var_dump(get_required_files() === get_included_files());
unlink('introspection_tail_dir/a.php');
unlink('introspection_tail_dir/b.php');
rmdir('introspection_tail_dir');

trait T1 {}
trait T2 { use T1; }
interface I {}
$traits = get_declared_traits();
var_dump(array_values(array_filter($traits, fn ($t) => in_array($t, ['T1', 'T2'], true))));
var_dump(in_array('I', $traits, true));

#[AllowDynamicProperties]
class A
{
    public $a = 1;
    protected $b = 2;
    private $c = 3;
    public $d;
    public int $typed;
}
#[AllowDynamicProperties]
class B extends A
{
    private $c = 4;
    public $e = 5;
}
$b = new B;
$b->dyn = 6;
$b->{'7'} = 'numeric';
unset($b->d);
$m = get_mangled_object_vars($b);
foreach ($m as $k => $v) {
    echo json_encode((string) $k), ' => ', json_encode($v), "\n";
}
var_dump($m === (array) $b);
var_dump(get_mangled_object_vars(new ArrayObject([1, 2])));
var_dump(get_mangled_object_vars(new stdClass));
var_dump(get_mangled_object_vars(function () {}));

$r = fopen('php://memory', 'r');
var_dump(is_int(get_resource_id($r)), get_resource_id($r) === (int) $r);
fclose($r);
var_dump(get_resource_id($r) === (int) $r);
try {
    get_resource_id(1);
} catch (\TypeError $e) {
    echo $e->getMessage(), "\n";
}
try {
    get_mangled_object_vars([]);
} catch (\TypeError $e) {
    echo $e->getMessage(), "\n";
}
