/**
 * Review Agent MCP Server
 */

import { MCPServer } from './server.js';
import { ReviewAgent } from '../agents/review/review-agent.js';

const agent = new ReviewAgent();
const server = new MCPServer(agent);

server.start().catch((error) => {
  console.error('Failed to start Review MCP Server:', error);
  process.exit(1);
});

