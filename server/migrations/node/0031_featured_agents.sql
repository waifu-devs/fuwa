-- The public agents the instance's admins feature in Browse, in order
-- (AdminService.SetFeaturedAgents). One made private keeps its place here
-- but doesn't show; deleting the agent deletes its row.
CREATE TABLE featured_agents (
  account_id TEXT NOT NULL PRIMARY KEY,
  position INTEGER NOT NULL
);
