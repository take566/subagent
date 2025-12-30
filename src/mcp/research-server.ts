/**
 * Research Agent MCP Server
 */

import { MCPServer } from './server.js';
import { ResearchAgent } from '../agents/research/research-agent.js';

const agent = new ResearchAgent();
const server = new MCPServer(agent);

server.start().catch((error) => {
  console.error('Failed to start Research MCP Server:', error);
  process.exit(1);
});

