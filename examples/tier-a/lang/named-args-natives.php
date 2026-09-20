<?php
// Named arguments to natives: php binds them against the arginfo names,
// fills skipped optional positions with the stub defaults, and refuses
// unknown names — `Error` before the frame exists, `ArgumentCountError`
// from inside it (a variadic native, a required parameter not passed).
// A by-reference named parameter still receives the caller's cell; the
// pass-through natives (`call_user_func`, `ReflectionClass::newInstance`,
// `Closure::call`, …) forward unknown names to the callable they invoke.
function t($f) { try { $f(); } catch (\Throwable $e) { echo get_class($e), ": ", $e->getMessage(), "\n", $e->getTraceAsString(), "\n"; } }
var_dump(json_decode('{"a":1}', true, flags: JSON_THROW_ON_ERROR));
var_dump(json_decode('[1]', flags: JSON_THROW_ON_ERROR));
var_dump(htmlspecialchars("<a href='x'>", double_encode: false));
var_dump(str_pad("x", 5, pad_type: STR_PAD_LEFT));
var_dump(array_slice([1,2,3,4], 1, preserve_keys: true));
var_dump(number_format(1234.567, decimals: 2));
var_dump(round(2.5, mode: PHP_ROUND_HALF_EVEN));
var_dump(count([1,[2]], mode: COUNT_RECURSIVE));
var_dump(implode(separator: ",", array: [1,2]));
var_dump(strlen(string: "abc"));
try { var_dump(json_decode(flags: 1)); } catch (\Throwable $e) { echo get_class($e), ": ", $e->getMessage(), "\n"; }
try { var_dump(strlen("x", nope: 1)); } catch (\Throwable $e) { echo get_class($e), ": ", $e->getMessage(), "\n"; }
try { var_dump(sprintf("%s", 1, nope: 1)); } catch (\Throwable $e) { echo get_class($e), ": ", $e->getMessage(), "\n"; }
try { var_dump(strlen("x", string: "y")); } catch (\Throwable $e) { echo get_class($e), ": ", $e->getMessage(), "\n"; }
try { var_dump(max(1, 2, nope: 3)); } catch (\Throwable $e) { echo get_class($e), ": ", $e->getMessage(), "\n"; }
$a = new ArrayObject([3,1,2]);
var_dump($a->getArrayCopy());
var_dump((new DateTime('2020-01-01 00:00:00', timezone: new DateTimeZone('UTC')))->format('c'));
var_dump(str_contains(haystack: "abc", needle: "b"));
var_dump(array_map(callback: fn($x) => $x * 2, array: [1,2]));
var_dump(array_filter([1,0,2], mode: ARRAY_FILTER_USE_KEY, callback: fn($k) => $k > 0));
var_dump(preg_split('/,/', 'a,,b', flags: PREG_SPLIT_NO_EMPTY));
var_dump(in_array("1", [1], strict: true));
var_dump(array_column([['a'=>1,'b'=>2]], index_key: 'b', column_key: 'a'));
var_dump(intdiv(num2: 2, num1: 9));
var_dump(str_replace(subject: "aXb", search: "X", replace: "-"));
var_dump(iterator_to_array(new ArrayIterator([5=>1]), preserve_keys: false));
var_dump(mb_substr("héllo", 1, encoding: "UTF-8"));
var_dump(base64_encode(string: "hi"));
var_dump(json_encode(["a"=>1], flags: JSON_PRETTY_PRINT));
var_dump(PDO::class);
var_dump(array_keys(["a"=>1,"b"=>1], filter_value: 1, strict: true));
var_dump(explode(limit: 2, string: "a-b-c", separator: "-"));
var_dump(trim(characters: "x", string: "xxaxx"));
$arr=[3,1,2]; var_dump(sort(array: $arr)); var_dump($arr); $m=null; var_dump(preg_match(pattern: "/b/", subject: "abc", matches: $m)); var_dump($m); var_dump(array_key_exists(key: "a", array: ["a"=>1])); var_dump(call_user_func_array(fn($a,$b)=>[$a,$b], ["b"=>2,"a"=>1])); t(fn() => array_map(fn($a) => $a, array: [1], arrays: [2]));

var_dump(call_user_func(fn($a, $b = 5) => [$a, $b], b: 2, a: 1));
var_dump(call_user_func_array(fn($a, $b = 5) => [$a, $b], ["b" => 2, "a" => 1]));
var_dump((new ReflectionClass('ArrayObject'))->newInstance(array: [1], flags: 2)->getFlags());
$f = fn($x, $y = 0) => $x - $y;
var_dump((new ReflectionFunction($f))->invoke(y: 1, x: 10));
var_dump($f->__invoke(y: 1, x: 10));
var_dump($f->call(new class {}, y: 1, x: 10));
var_dump(json_decode('{"a":1}', true, flags: JSON_THROW_ON_ERROR, depth: 3));
var_dump(json_decode(json: '[1]'));
try { var_dump(json_decode(json: '[1]', json: '[2]')); } catch (\Throwable $e) { echo get_class($e), ": ", $e->getMessage(), "\n"; }

t(fn() => strlen("x", nope: 1));
t(fn() => strlen("x", string: "y"));
t(fn() => json_decode(flags: 1));
t(fn() => sprintf("%s", 1, nope: 1));
t(fn() => (new ArrayObject([]))->append(nope: 1));
t(fn() => (new ArrayObject([]))->setFlags(nope: 1));
t(fn() => new ArrayObject(flags: 1, nope: 2));
t(fn() => array_map(fn($a) => $a, array: [1], arrays: [2]));

function f($a, ...$r) { throw new Exception("x"); }
try { f(1, 2, k: 3); } catch (Exception $e) { echo $e->getTraceAsString(), "\n"; var_dump($e->getTrace()[0]['args']); }
