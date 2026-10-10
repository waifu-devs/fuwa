-- Messages name people as @username: finding who that is (mention_user_ids)
-- shouldn't read every user in a big server.
CREATE INDEX users_by_username ON users (username);
