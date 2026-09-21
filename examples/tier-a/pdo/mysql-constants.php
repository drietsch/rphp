<?php
// Without a MySQL client, php still declares the driver's constants and its
// 8.4 subclass, and `mysql:` without a driver is "could not find driver".
var_dump(PDO::MYSQL_ATTR_INIT_COMMAND, PDO::MYSQL_ATTR_USE_BUFFERED_QUERY, PDO::MYSQL_ATTR_SSL_VERIFY_SERVER_CERT);
var_dump(Pdo\Mysql::ATTR_INIT_COMMAND, class_exists('Pdo\Mysql'), is_subclass_of('Pdo\Mysql', 'PDO'));
var_dump(in_array('sqlite', PDO::getAvailableDrivers(), true));
