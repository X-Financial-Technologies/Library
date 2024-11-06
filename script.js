// script.js
let currentPath = '.';

async function displayContents(dirPath = '.') {
   currentPath = dirPath;
   const table = document.getElementById('repo-table');
   table.innerHTML = '';
   
   // Add back button if not in root
   if (dirPath !== '.') {
       const parentPath = dirPath.split('/').slice(0, -1).join('/') || '.';
       const backRow = table.insertRow();
       const backCell = backRow.insertCell(0);
       backCell.colSpan = 2;
       backCell.innerHTML = `<a href="#" onclick="displayContents('${parentPath}'); return false">📁 ..</a>`;
   }
   
   try {
       const response = await fetch(`/api/files/${dirPath}`);
       const items = await response.json();
       
       items.forEach((item, index) => {
           const row = table.insertRow();
           const cell1 = row.insertCell(0);
           const cell2 = row.insertCell(1);
           
           cell1.textContent = `${index + 1}.`;
           
           if (item.isDirectory) {
               cell2.innerHTML = `<a href="#" onclick="displayContents('${item.path}'); return false">📁 ${item.name}</a>`;
           } else {
               cell2.innerHTML = `<a href="${item.path}">📄 ${item.name}</a>`;
           }
       });
   } catch (error) {
       console.error('Error:', error);
   }
}

displayContents();