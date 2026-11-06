import { defineConfig } from 'vitest/config'

// Pure logic tests only: the suite runs in Node without a DOM. Hooks and components
// would need jsdom/testing-library, which this project has deliberately not added yet.
export default defineConfig({
  test: {
    environment: 'node',
    include: ['src/**/*.test.ts'],
  },
})
