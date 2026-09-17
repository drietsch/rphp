<?php
// Namespaces: declarations register under their FQN, `use` / `use function` /
// `use const` / group use / aliases resolve at compile time, `namespace\X` is
// relative, magic constants see the namespace, `::class` folds, and the
// global block at the end reaches namespaced symbols through their FQN.
// Global functions and constants are written `\strlen()` / `\PHP_EOL` inside
// the namespaces: php's unqualified two-step lookup (`App\strlen` then
// `strlen`) is a runtime feature this snippet does not depend on.

namespace App\Config {
    const VERSION = '1.2.3';
    const DEBUG = false;

    function describe() {
        return __NAMESPACE__ . ' ' . VERSION . ' ' . namespace\VERSION . ' ' . \App\Config\VERSION;
    }
}

namespace App\Util {
    function fmt($label, $value) {
        return \sprintf('%s=%s', $label, $value);
    }

    function twice($x) {
        return $x * 2;
    }

    function where() {
        return __FUNCTION__;
    }

    function inner() {
        $f = function () { return __FUNCTION__; };
        return $f();
    }
}

namespace App\Models {
    class User {
        public $name;
        function __construct($name) { $this->name = $name; }
        function who() { return __CLASS__ . '|' . __METHOD__ . '|' . __FUNCTION__ . '|' . __NAMESPACE__; }
        function self_name() { return self::class; }
    }

    class Post {
        public $title = 'untitled';
        function tag() { return static::class; }
    }
}

namespace App {
    use App\Models\User;
    use App\Models\{Post as Article, User as Person};
    use function App\Util\{fmt, twice};
    use function App\Util\where as location;
    use const App\Config\VERSION;
    use const App\Config\DEBUG as IS_DEBUG;

    echo "-- names --", \PHP_EOL;
    echo __NAMESPACE__, \PHP_EOL;                    // App
    echo User::class, \PHP_EOL;                      // App\Models\User
    echo Article::class, \PHP_EOL;                   // App\Models\Post
    echo Person::class, \PHP_EOL;                    // App\Models\User
    echo Models\Post::class, \PHP_EOL;               // App\Models\Post (qualified via the namespace)
    echo namespace\Models\Post::class, \PHP_EOL;     // App\Models\Post (relative)
    echo \App\Models\Post::class, \PHP_EOL;          // fully qualified
    echo Unknown\Thing::class, \PHP_EOL;             // App\Unknown\Thing (no lookup for ::class)

    echo "-- objects --", \PHP_EOL;
    $u = new User('ann');
    $a = new Article();
    $p = new Person('bob');
    echo $u->who(), \PHP_EOL;
    echo $u->self_name(), \PHP_EOL;
    echo $a->tag(), \PHP_EOL;
    echo $p->name, ' ', $a->title, \PHP_EOL;
    echo \get_class($u), ' ', $u::class, \PHP_EOL;
    \var_dump($u instanceof User, $u instanceof Person, $a instanceof User, $u instanceof \App\Models\User);
    \var_dump(\class_exists('App\Models\User'), \class_exists('app\models\user'), \class_exists('User'));

    echo "-- functions --", \PHP_EOL;
    echo fmt('a', 1), \PHP_EOL;                      // App\Util\fmt through the import
    echo twice(21), \PHP_EOL;
    echo location(), \PHP_EOL;                       // App\Util\where
    echo Util\where(), \PHP_EOL;                     // qualified through the namespace
    echo namespace\Util\twice(4), \PHP_EOL;          // relative
    echo \App\Util\inner(), \PHP_EOL;                // {closure:App\Util\inner():33}
    \var_dump(\function_exists('App\Util\fmt'), \function_exists('APP\UTIL\FMT'), \function_exists('fmt'));

    echo "-- constants --", \PHP_EOL;
    echo VERSION, \PHP_EOL;                          // App\Config\VERSION through the import
    \var_dump(IS_DEBUG);
    echo Config\VERSION, \PHP_EOL;                   // qualified
    echo namespace\Config\VERSION, \PHP_EOL;         // relative
    echo \App\Config\describe(), \PHP_EOL;
    \var_dump(\defined('App\Config\VERSION'), \defined('App\Config\version'));

    const LOCAL = 'local';
    echo LOCAL, ' ', namespace\LOCAL, ' ', \App\LOCAL, \PHP_EOL;

    echo "-- magic in a namespaced function --", \PHP_EOL;
    function report() {
        return __FUNCTION__ . '|' . __NAMESPACE__;
    }
    echo report(), \PHP_EOL;
    echo \App\report(), \PHP_EOL;
}

namespace Other {
    // A second block: imports do not leak, and the same short names are new symbols.
    class User {
        function who() { return __CLASS__; }
    }
    function report() {
        return __FUNCTION__;
    }
    echo "-- second block --", \PHP_EOL;
    echo __NAMESPACE__, \PHP_EOL;
    echo (new User)->who(), \PHP_EOL;                // Other\User
    echo report(), \PHP_EOL;                         // Other\report
    echo \App\report(), \PHP_EOL;                    // App\report
    echo \App\Models\User::class, \PHP_EOL;
}

namespace {
    echo "-- global --", PHP_EOL;
    var_dump(__NAMESPACE__);
    echo App\Util\fmt('global', 'call'), PHP_EOL;    // namespaced function from the global namespace
    echo \App\Util\twice(5), PHP_EOL;
    echo App\Config\VERSION, ' ', \App\LOCAL, PHP_EOL;
    $x = new App\Models\User('cid');
    echo $x->who(), PHP_EOL;
    echo get_class(new Other\User), PHP_EOL;
    var_dump($x instanceof App\Models\User, $x instanceof Other\User);
    echo strlen('plain'), PHP_EOL;
    var_dump(function_exists('App\report'), function_exists('report'));
}
