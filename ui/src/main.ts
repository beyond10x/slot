import { createApp } from 'vue';
import App from './App.vue';
import './style.css';
import { start } from './store.ts';

void start();
createApp(App).mount('#app');
