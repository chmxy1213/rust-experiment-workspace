<script setup>
import { ref, onMounted, computed } from 'vue'
import axios from 'axios'

const logs = ref([])
const currentLog = ref(null)
const logLines = ref([])

const fetchLogs = async () => {
  try {
    const response = await axios.get('/api/logs')
    logs.value = response.data
  } catch (error) {
    console.error('Error fetching logs:', error)
  }
}

const groupedLogs = computed(() => {
  const groups = {}
  logs.value.forEach(log => {
    if (!groups[log.agent_name]) {
      groups[log.agent_name] = []
    }
    groups[log.agent_name].push(log)
  })
  return groups
})

const viewLog = async (path) => {
  try {
    const response = await axios.get(`/api/logs/content?path=${encodeURIComponent(path)}`)
    currentLog.value = path
    logLines.value = response.data.split('\n')
  } catch (error) {
    console.error('Error fetching log content:', error)
    alert('Failed to load log content')
  }
}

const getLogClass = (line) => {
  const upperLine = line.toUpperCase()
  if (upperLine.includes('ERROR')) return 'text-red-500'
  if (upperLine.includes('WARN')) return 'text-yellow-500'
  if (upperLine.includes('INFO')) return 'text-blue-500'
  if (upperLine.includes('TRACE')) return 'text-violet-500'
  if (upperLine.includes('DEBUG')) return 'text-emerald-500'
  return ''
}

onMounted(() => {
  fetchLogs()
})
</script>

<template>
  <div class="flex flex-col h-screen bg-gray-100 text-gray-800">
    <header class="bg-white shadow-sm z-10 py-4 px-6">
      <h1 class="text-2xl font-bold text-gray-900">Log Server</h1>
    </header>
    
    <div class="flex flex-1 overflow-hidden">
      <!-- Left Sidebar: Log List -->
      <div class="w-1/3 md:w-1/4 lg:w-1/5 bg-white border-r border-gray-200 overflow-y-auto flex flex-col">
        <div class="p-4 border-b border-gray-200 bg-gray-50 flex justify-between items-center sticky top-0 z-10">
          <h2 class="text-lg font-semibold">Log Files</h2>
          <button @click="fetchLogs" class="text-gray-500 hover:text-blue-500" title="Refresh">
            <svg class="w-5 h-5" fill="none" stroke="currentColor" viewBox="0 0 24 24" xmlns="http://www.w3.org/2000/svg"><path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M4 4v5h.582m15.356 2A8.001 8.001 0 004.582 9m0 0H9m11 11v-5h-.581m0 0a8.003 8.003 0 01-15.357-2m15.357 2H15"></path></svg>
          </button>
        </div>
        
        <div v-if="logs.length === 0" class="p-4 text-gray-500 text-center italic">
          No logs found.
        </div>
        
        <div class="flex-1 overflow-y-auto">
          <div v-for="(agentLogs, agentName) in groupedLogs" :key="agentName" class="border-b border-gray-100 last:border-0">
            <div class="bg-gray-100 px-3 py-2 text-sm font-semibold text-gray-700 flex items-center sticky top-0">
              <svg class="w-4 h-4 mr-1.5 text-gray-500" fill="none" stroke="currentColor" viewBox="0 0 24 24" xmlns="http://www.w3.org/2000/svg"><path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M5 12h14M5 12a2 2 0 01-2-2V6a2 2 0 012-2h14a2 2 0 012 2v4a2 2 0 01-2 2M5 12a2 2 0 00-2 2v4a2 2 0 002 2h14a2 2 0 002-2v-4a2 2 0 00-2-2m-2-4h.01M17 16h.01"></path></svg>
              {{ agentName }}
              <span class="ml-auto bg-gray-200 text-gray-600 text-xs px-1.5 py-0.5 rounded">{{ agentLogs.length }}</span>
            </div>
            <ul class="divide-y divide-gray-50">
              <li v-for="log in agentLogs" :key="log.path" 
                  @click="viewLog(log.path)"
                  :class="['px-3 py-2.5 cursor-pointer hover:bg-blue-50 transition-colors duration-150', currentLog === log.path ? 'bg-blue-100 border-l-4 border-blue-500' : 'border-l-4 border-transparent']">
                <p class="text-sm font-medium text-gray-900 truncate" :title="log.filename">{{ log.filename }}</p>
                <p class="text-xs text-gray-500 truncate mt-0.5" :title="log.path">{{ log.path.split('/').slice(1).join('/') }}</p>
              </li>
            </ul>
          </div>
        </div>
      </div>

      <!-- Right Content: Log Viewer -->
      <div class="flex-1 flex flex-col bg-gray-50 overflow-hidden">
        <div v-if="currentLog" class="flex-1 flex flex-col h-full">
          <div class="bg-white px-6 py-3 border-b border-gray-200 flex justify-between items-center shadow-sm z-10">
            <h2 class="text-lg font-semibold text-gray-800 truncate pr-4" :title="currentLog">{{ currentLog }}</h2>
            <button @click="viewLog(currentLog)" class="text-gray-500 hover:text-blue-500 flex items-center text-sm font-medium" title="Reload Log">
              <svg class="w-4 h-4 mr-1" fill="none" stroke="currentColor" viewBox="0 0 24 24" xmlns="http://www.w3.org/2000/svg"><path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M4 4v5h.582m15.356 2A8.001 8.001 0 004.582 9m0 0H9m11 11v-5h-.581m0 0a8.003 8.003 0 01-15.357-2m15.357 2H15"></path></svg>
              Reload
            </button>
          </div>
          <div class="flex-1 bg-gray-900 text-gray-100 p-4 overflow-auto font-mono text-sm shadow-inner">
            <div v-for="(line, index) in logLines" :key="index" :class="['whitespace-pre-wrap break-all hover:bg-gray-800 px-1 rounded', getLogClass(line)]">{{ line || ' ' }}</div>
          </div>
        </div>
        
        <div v-else class="flex-1 flex items-center justify-center text-gray-400 flex-col">
          <svg class="w-16 h-16 mb-4 text-gray-300" fill="none" stroke="currentColor" viewBox="0 0 24 24" xmlns="http://www.w3.org/2000/svg"><path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M9 12h6m-6 4h6m2 5H7a2 2 0 01-2-2V5a2 2 0 012-2h5.586a1 1 0 01.707.293l5.414 5.414a1 1 0 01.293.707V19a2 2 0 01-2 2z"></path></svg>
          <p class="text-lg font-medium">Select a log file from the left to view its contents</p>
        </div>
      </div>
    </div>
  </div>
</template>

