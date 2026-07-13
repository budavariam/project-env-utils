const apiUrl = import.meta.env.VITE_API_URL || 'http://localhost:3000';

document.querySelector('#app').innerHTML = `
  <h1>Demo UI</h1>
  <p>API: <code>${apiUrl}</code> (${import.meta.env.MODE})</p>
`;
