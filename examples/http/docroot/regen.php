<?php
// A handler that deletes the cookie on destroy, as Symfony's does; php sends
// the regenerated id's cookie once, dropping the deletion.
class H extends SessionHandler { function destroy(string $id): bool { setcookie(session_name(), '', 0, '/'); return parent::destroy($id); } }
session_set_save_handler(new H, true);
session_start();
$_SESSION['x'] = 1;
session_regenerate_id(true);
foreach (headers_list() as $h) echo $h, "\n";
