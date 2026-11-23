-- 003_admin_csrf.sql — per-session CSRF token for the admin panel,
-- stored on the browser_log (session) row, like PHP's $_SESSION['csrf_token'].
ALTER TABLE pp_browser_log ADD COLUMN csrf_token TEXT NOT NULL DEFAULT '--';
