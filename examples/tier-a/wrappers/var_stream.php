<?php
// The classic memory-backed var:// wrapper, with every call it gets traced.
class VarStream {
    static $vars = [];
    public $context;
    private $name;
    private $pos = 0;
    function stream_open($path, $mode, $options, &$opened_path) {
        echo "open $path $mode $options ", gettype($this->context), "\n";
        $this->name = parse_url($path, PHP_URL_HOST);
        if (!isset(self::$vars[$this->name])) {
            if ($mode[0] === 'r') return false;
            self::$vars[$this->name] = '';
        }
        if ($mode[0] === 'w') self::$vars[$this->name] = '';
        if ($mode[0] === 'a') $this->pos = strlen(self::$vars[$this->name]);
        return true;
    }
    function stream_read($count) {
        echo "read $count\n";
        $ret = substr(self::$vars[$this->name], $this->pos, $count);
        $this->pos += strlen($ret);
        return $ret;
    }
    function stream_write($data) {
        echo "write ", strlen($data), "\n";
        $v = &self::$vars[$this->name];
        $v = substr($v, 0, $this->pos) . $data . substr($v, $this->pos + strlen($data));
        $this->pos += strlen($data);
        return strlen($data);
    }
    function stream_tell() { echo "tell\n"; return $this->pos; }
    function stream_eof() { echo "eof\n"; return $this->pos >= strlen(self::$vars[$this->name]); }
    function stream_seek($offset, $whence) {
        echo "seek $offset $whence\n";
        $len = strlen(self::$vars[$this->name]);
        $to = match ($whence) { SEEK_SET => $offset, SEEK_CUR => $this->pos + $offset, SEEK_END => $len + $offset };
        if ($to < 0) return false;
        $this->pos = $to;
        return true;
    }
    function stream_stat() { echo "stat\n"; return ['size' => strlen(self::$vars[$this->name])]; }
    function stream_flush() { echo "flush\n"; return true; }
    function stream_close() { echo "close\n"; }
    function stream_truncate($size) { echo "truncate $size\n"; self::$vars[$this->name] = substr(str_pad(self::$vars[$this->name], $size, "\0"), 0, $size); return true; }
    function stream_lock($op) { echo "lock $op\n"; return true; }
    function url_stat($path, $flags) {
        echo "url_stat $path $flags\n";
        $n = parse_url($path, PHP_URL_HOST);
        return isset(self::$vars[$n]) ? ['mode' => 0100644, 'size' => strlen(self::$vars[$n])] : false;
    }
    function unlink($path) { echo "unlink $path\n"; unset(self::$vars[parse_url($path, PHP_URL_HOST)]); return true; }
    function rename($from, $to) {
        echo "rename $from $to\n";
        self::$vars[parse_url($to, PHP_URL_HOST)] = self::$vars[parse_url($from, PHP_URL_HOST)];
        unset(self::$vars[parse_url($from, PHP_URL_HOST)]);
        return true;
    }
}

var_dump(stream_wrapper_register('var', 'VarStream'));
var_dump(in_array('var', stream_get_wrappers()));

VarStream::$vars["myvar"] = "line one\nline two\nline three\n";
$fp = fopen('var://myvar', 'r+');
var_dump(fgets($fp));
var_dump(fread($fp, 4));
var_dump(ftell($fp));
var_dump(fseek($fp, 0));
var_dump(fgets($fp));
var_dump(feof($fp));
var_dump(stream_get_contents($fp));
var_dump(feof($fp));
var_dump(fseek($fp, 5));
var_dump(fwrite($fp, "ONE"));
var_dump(fflush($fp));
var_dump(ftruncate($fp, 12));
var_dump(flock($fp, LOCK_EX), flock($fp, LOCK_UN));
var_dump(fstat($fp)['size']);
$meta = stream_get_meta_data($fp);
var_dump($meta['wrapper_type'], $meta['stream_type'], $meta['mode'], $meta['uri'], $meta['seekable'], get_class($meta['wrapper_data']));
var_dump(fclose($fp));
var_dump(VarStream::$vars["myvar"]);

echo "-- whole-file functions\n";
var_dump(file_put_contents('var://other', "a\nb\nc\n"));
var_dump(file_get_contents('var://other'));
var_dump(file('var://other', FILE_IGNORE_NEW_LINES));
var_dump(readfile('var://other'));
var_dump(file_put_contents('var://other', ['x', 'y'], FILE_APPEND));
var_dump(VarStream::$vars["other"]);
var_dump(file_get_contents('var://nope'));

echo "-- a large read goes out in chunks\n";
VarStream::$vars["big"] = str_repeat("0123456789", 2000);
$fp = fopen('var://big', 'r');
var_dump(strlen(fread($fp, 100)));
var_dump(strlen(fread($fp, 20000)));
var_dump(strlen(stream_get_contents($fp)));
fclose($fp);
$fp = fopen('var://big', 'w');
var_dump(fwrite($fp, str_repeat('z', 10000)));
fclose($fp);
var_dump(strlen(VarStream::$vars["big"]));

echo "-- stat and friends\n";
var_dump(file_exists('var://myvar'), is_file('var://myvar'), is_dir('var://myvar'), filesize('var://myvar'));
clearstatcache();
var_dump(file_exists('var://gone'));
var_dump(rename('var://myvar', 'var://moved'));
var_dump(isset(VarStream::$vars["myvar"]), VarStream::$vars["moved"]);
var_dump(unlink('var://moved'));
var_dump(isset(VarStream::$vars["moved"]));
var_dump(copy('var://other', 'var://copied'), VarStream::$vars["copied"]);
