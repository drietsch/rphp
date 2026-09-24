<?php
// The calls behind flock/ftruncate/stream_set_*/stream_select, bad return
// values, and the instance's lifecycle for one-shot operations.
class T {
    public $context;
    public $data = "0123456789";
    public $pos = 0;
    function __construct() { echo "ctor ", gettype($this->context), "\n"; }
    function __destruct() { echo "dtor\n"; }
    function stream_open($path, $mode, $options, &$opened) {
        echo "open $path $mode $options\n";
        if (str_contains($path, 'throw')) throw new Exception("boom");
        return true;
    }
    function stream_read($n) {
        echo "read $n\n";
        if ($this->data === 'more') return str_repeat('x', $n + 5);
        if ($this->data === 'int') return 123;
        $r = substr($this->data, $this->pos, $n); $this->pos += strlen($r); return $r;
    }
    function stream_eof() { return $this->pos >= 10; }
    function stream_write($d) { echo "write ", strlen($d), "\n"; return $this->data === 'more' ? strlen($d) + 3 : strlen($d); }
    function stream_lock($op) { echo "lock $op\n"; return true; }
    function stream_truncate($n) { echo "truncate $n\n"; return true; }
    function stream_set_option($o, $a, $b) { echo "set_option $o ", var_export($a, true), " ", var_export($b, true), "\n"; return true; }
    function stream_cast($as) { echo "cast $as\n"; return false; }
    function stream_seek($o, $w) { echo "seek $o $w\n"; if ($o < 0) return false; $this->pos = $o; return true; }
    function stream_tell() { echo "tell\n"; return $this->pos; }
    function stream_close() { echo "close\n"; }
    function url_stat($p, $f) { echo "url_stat $p $f\n"; return ['mode' => 040755]; }
    function unlink($p) { echo "unlink $p\n"; return true; }
    function mkdir($p, $m, $o) { echo "mkdir $p $m $o\n"; return true; }
    function rmdir($p, $o) { echo "rmdir $p $o\n"; return true; }
    function stream_metadata($p, $o, $v) { echo "meta $p $o ", json_encode($v), "\n"; return true; }
}
stream_wrapper_register('tpl', 'T');
try { fopen('tpl://throw', 'r'); } catch (Exception $e) { echo get_class($e), ": ", $e->getMessage(), "\n"; }
$f = fopen('tpl://a', 'rb+', false, stream_context_create());
$o = stream_get_meta_data($f)['wrapper_data'];
var_dump(fread($f, 3), fread($f, 3), ftell($f));
var_dump(fseek($f, 1, SEEK_CUR), ftell($f));
var_dump(fseek($f, -1), ftell($f));
var_dump(fseek($f, 0, SEEK_END), ftell($f));
$o->data = 'more'; $o->pos = 0; fseek($f, 0);
var_dump(strlen(fread($f, 3)));
var_dump(fwrite($f, "abc"));
$o->data = 'int'; fseek($f, 0);
var_dump(fread($f, 3));
var_dump(flock($f, LOCK_SH), flock($f, LOCK_EX | LOCK_NB), flock($f, LOCK_UN));
var_dump(ftruncate($f, 5));
try { ftruncate($f, -1); } catch (\ValueError $e) { echo $e->getMessage(), "\n"; }
var_dump(stream_set_blocking($f, false));
var_dump(stream_set_timeout($f, 5, 10));
var_dump(stream_set_write_buffer($f, 0));
var_dump(stream_set_write_buffer($f, 100));
var_dump(stream_set_read_buffer($f, 0));
$r = [$f]; $w = null; $e = null;
try { var_dump(stream_select($r, $w, $e, 0)); } catch (\ValueError $ex) { echo $ex->getMessage(), "\n"; }
var_dump(stream_is_local($f));
var_dump(fclose($f));

echo "-- one-shot operations\n";
var_dump(is_dir('tpl://d'));
var_dump(unlink('tpl://a'));
var_dump(mkdir('tpl://a'));
var_dump(mkdir('tpl://a/b/c', 0700, true));
var_dump(rmdir('tpl://a'));
var_dump(touch('tpl://a'));
var_dump(touch('tpl://a', 5));
var_dump(touch('tpl://a', 5, 6));
var_dump(chmod('tpl://a', 0755));
var_dump(chown('tpl://a', 'root'), chown('tpl://a', 0));
var_dump(chgrp('tpl://a', 'wheel'), chgrp('tpl://a', 0));
echo "done\n";
