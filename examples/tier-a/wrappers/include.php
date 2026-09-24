<?php
// include/require of a url a user wrapper serves (vfsStream-style).
ini_set('include_path', '.');
class Tpl {
    public $context;
    static $files = [
        'tpl://a.php' => "<?php echo 'in a ', __FILE__, \"\\n\"; return 42;",
        'tpl://b.php' => "hello <?= 1 + 1 ?>\n",
        'tpl://c.php' => "<?php function from_c() { return 'c'; }",
    ];
    private $d; private $p = 0;
    function stream_open($path, $mode, $options, &$opened) {
        echo "open $path $mode $options\n";
        if (!isset(self::$files[$path])) return false;
        $this->d = self::$files[$path]; return true;
    }
    function stream_read($n) { echo "read $n\n"; $r = substr($this->d, $this->p, $n); $this->p += strlen($r); return $r; }
    function stream_eof() { return $this->p >= strlen($this->d); }
    function stream_set_option($o, $a, $b) { echo "set_option $o $a $b\n"; return false; }
    function stream_stat() { echo "stat\n"; return ['size' => strlen($this->d)]; }
    function stream_close() { echo "close\n"; }
}
stream_wrapper_register('tpl', 'Tpl');
var_dump(include 'tpl://a.php');
var_dump(include 'tpl://b.php');
var_dump(require_once 'tpl://c.php');
var_dump(require_once 'tpl://c.php');
var_dump(from_c());
var_dump(include 'tpl://missing.php');

class NoStat {
    public $context;
    private $d = '<?php echo "Hello World\n";?>'; private $p = 0;
    function stream_open($path, $mode, $options, &$opened) { return true; }
    function stream_read($n) { $r = substr($this->d, $this->p, $n); $this->p += strlen($r); return $r; }
    function stream_eof() { return $this->p >= strlen($this->d); }
}
stream_wrapper_register('url1', 'NoStat', STREAM_IS_URL);
stream_wrapper_register('loc1', 'NoStat');
var_dump(stream_is_local('url1://x'), stream_is_local('loc1://x'));
include 'url1://hello';
include 'loc1://hello';
try { require 'tpl://missing.php'; } catch (\Error $e) { echo get_class($e), ": ", preg_replace("/include_path='.*'/", "include_path=…", $e->getMessage()), "\n"; }
