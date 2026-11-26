import { createApp } from 'vue';
import App from './App.vue';
import './style.css';
import './uxui/dense.css';
import './uxui/slot-jobs.css';
import { start } from './store.ts';

void start();
createApp(App).mount('#app');
